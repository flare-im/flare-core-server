//! Explicit accepted initial v1 -> v2 transition; no SQLx history adoption.
use crate::{digest, ensure, model};
use flare_server_core::error::{FlareError, Result};
use sqlx::{Connection, PgConnection};
/// Returns true only when this transaction commits the accepted forward DDL.
pub async fn upgrade(connection: &mut PgConnection) -> Result<bool> {
    upgrade_sql(connection, model::UPGRADE_SQL).await
}
async fn upgrade_sql(connection: &mut PgConnection, ddl: &str) -> Result<bool> {
    let mut tx = connection
        .begin()
        .await
        .map_err(|_| FlareError::system("Core upgrade transaction unavailable"))?;
    let work=async {
        model::begin_checks(&mut tx).await?;
        model::lock_tables(&mut tx).await?;
        let (version,marker)=model::marker(&mut tx).await?;
        if version==2 {model::verify_current(&mut tx).await?;return Ok(false);}
        ensure(version==1 && marker==model::PREVIOUS_DIGEST,"Core initial upgrade baseline is not accepted")?;
        model::shape(&mut tx,1).await?;
        sqlx::query("SELECT set_config('flare.core_initial_target_sha256',$1,true)").bind(digest()).execute(&mut *tx).await.map_err(|_|FlareError::system("Core upgrade target unavailable"))?;
        sqlx::raw_sql(ddl).execute(&mut *tx).await.map_err(|_|FlareError::system("Core initial upgrade DDL failed; transaction rolled back"))?;
        sqlx::query("INSERT INTO public.core_initial_history(to_version,from_version,from_sha256,to_sha256,transition,ddl_sha256) VALUES(2,1,$1,$2,'application-media-v2',$3)").bind(model::PREVIOUS_DIGEST).bind(digest()).bind(model::sha(model::UPGRADE_SQL)).execute(&mut *tx).await.map_err(|_|FlareError::system("Core upgrade provenance failed; transaction rolled back"))?;
        model::verify_current(&mut tx).await?;
        Ok(true)
    }.await;
    match work {
        Ok(changed) => {
            tx.commit().await.map_err(|_|FlareError::system("Core upgrade commit unconfirmed; inspect and repeat the identical explicit command"))?;
            Ok(changed)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    #[test]
    fn archived_source_and_upgrade_are_explicit() {
        assert_eq!(model::previous_digest(), model::PREVIOUS_DIGEST);
        assert_ne!(digest(), model::PREVIOUS_DIGEST);
        assert!(model::UPGRADE_SQL.contains("WHERE version = 1"));
        assert!(!model::UPGRADE_SQL.contains("_sqlx_migrations"));
    }
    #[tokio::test]
    #[ignore = "requires an owned actual TimescaleDB Core initial upgrade fixture"]
    async fn initial_upgrade_validates_source_authority_and_commits_atomically() {
        let url = std::env::var("FLARE_CORE_INITIAL_UPGRADE_TEST_URL")
            .expect("owned TimescaleDB fixture required");
        let mut admin = PgConnection::connect(&url).await.unwrap();
        let database:String=sqlx::query_scalar("SELECT current_database() WHERE current_database() LIKE 'flare_core_initial_test_%' AND shobj_description((SELECT oid FROM pg_database WHERE datname=current_database()),'pg_database')='flare-core-initial-owned/v1'").fetch_one(&mut admin).await.unwrap();
        assert!(database.starts_with("flare_core_initial_test_"));
        assert!(upgrade(&mut admin).await.is_err());
        let options = sqlx::postgres::PgConnectOptions::from_str(&url).unwrap();
        let mut owner = PgConnection::connect_with(&options.clone().username("core_initialize"))
            .await
            .unwrap();
        sqlx::raw_sql(model::PREVIOUS_SQL)
            .execute(&mut owner)
            .await
            .unwrap();
        sqlx::raw_sql(model::PREVIOUS_SEEDS)
            .execute(&mut owner)
            .await
            .unwrap();
        sqlx::query("INSERT INTO public.core_initial_schema(version,schema_sha256) VALUES(1,$1)")
            .bind(model::PREVIOUS_DIGEST)
            .execute(&mut owner)
            .await
            .unwrap();
        sqlx::query("INSERT INTO public.media_assets(tenant_id,file_id,owner_id,file_name,mime_type,file_size,url,metadata) VALUES('fixture','preserved','actual-user','fixture','text/plain',1,'','{\"owner_kind\":\"application\",\"purpose\":\"avatar\"}')").execute(&mut owner).await.unwrap();
        sqlx::query(
            "UPDATE public.capability_service_settings SET global_enabled=false WHERE id=1",
        )
        .execute(&mut owner)
        .await
        .unwrap();
        let original: serde_json::Value = sqlx::query_scalar(
            "SELECT to_jsonb(a) FROM public.media_assets a WHERE file_id='preserved'",
        )
        .fetch_one(&mut owner)
        .await
        .unwrap();
        assert!(crate::initialize(&mut owner).await.is_err());
        let db = database.replace('"', "\"\"");
        sqlx::raw_sql(&format!(
            "GRANT CREATE ON DATABASE \"{db}\" TO core_runtime"
        ))
        .execute(&mut admin)
        .await
        .unwrap();
        assert!(upgrade(&mut owner).await.is_err());
        sqlx::raw_sql(&format!(
            "REVOKE CREATE ON DATABASE \"{db}\" FROM core_runtime"
        ))
        .execute(&mut admin)
        .await
        .unwrap();
        assert!(
            !sqlx::query_scalar::<_, bool>(
                "SELECT has_database_privilege('core_runtime',current_database(),'CREATE')"
            )
            .fetch_one(&mut owner)
            .await
            .unwrap()
        );
        // Only the cluster administrator owns the accepted extensions. The
        // initializer cannot hide application relations as extension members.
        for extension in ["timescaledb", "plpgsql"] {
            let denied = sqlx::raw_sql(&format!(
                "ALTER EXTENSION {extension} ADD TABLE public.media_assets"
            ))
            .execute(&mut owner)
            .await
            .unwrap_err();
            assert_eq!(
                denied.as_database_error().and_then(|e| e.code()).as_deref(),
                Some("42501")
            );
        }
        // A newly installed trusted extension is itself outside the reviewed
        // inventory, even when its owner adds a table that member filters omit.
        sqlx::raw_sql("CREATE EXTENSION pg_trgm; CREATE TABLE public.hidden_upgrade(value int); ALTER EXTENSION pg_trgm ADD TABLE public.hidden_upgrade;").execute(&mut owner).await.unwrap();
        assert!(upgrade(&mut owner).await.is_err());
        sqlx::raw_sql("DROP EXTENSION pg_trgm CASCADE;")
            .execute(&mut owner)
            .await
            .unwrap();
        sqlx::query("UPDATE public.core_initial_schema SET schema_sha256=repeat('0',64)")
            .execute(&mut owner)
            .await
            .unwrap();
        assert!(upgrade(&mut owner).await.is_err());
        sqlx::query("UPDATE public.core_initial_schema SET schema_sha256=$1")
            .bind(model::PREVIOUS_DIGEST)
            .execute(&mut owner)
            .await
            .unwrap();
        sqlx::query("CREATE INDEX unreviewed ON public.media_assets(file_name)")
            .execute(&mut owner)
            .await
            .unwrap();
        assert!(upgrade(&mut owner).await.is_err());
        sqlx::query("DROP INDEX public.unreviewed")
            .execute(&mut owner)
            .await
            .unwrap();
        let mut drift = owner.begin().await.unwrap();
        sqlx::query("GRANT UPDATE(schema_sha256) ON public.core_initial_schema TO core_runtime")
            .execute(&mut *drift)
            .await
            .unwrap();
        assert!(upgrade(&mut drift).await.is_err());
        drift.rollback().await.unwrap();
        sqlx::query("GRANT USAGE ON SCHEMA _timescaledb_catalog TO core_runtime")
            .execute(&mut admin)
            .await
            .unwrap();
        assert!(upgrade(&mut owner).await.is_err());
        sqlx::query("REVOKE USAGE ON SCHEMA _timescaledb_catalog FROM core_runtime")
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE public.media_assets OWNER TO core_runtime")
            .execute(&mut admin)
            .await
            .unwrap();
        assert!(upgrade(&mut owner).await.is_err());
        sqlx::query("ALTER TABLE public.media_assets OWNER TO core_initialize")
            .execute(&mut admin)
            .await
            .unwrap();
        // OWNER changes rewrite ACL grantors; restore the canonical effective ACL.
        sqlx::raw_sql("REVOKE ALL ON public.media_assets FROM core_runtime; GRANT SELECT,INSERT,UPDATE,DELETE ON public.media_assets TO core_runtime;").execute(&mut owner).await.unwrap();
        assert!(
            upgrade_sql(&mut owner, &format!("{}\nSELECT 1/0;", model::UPGRADE_SQL))
                .await
                .is_err()
        );
        assert_eq!(
            model::marker(&mut owner).await.unwrap(),
            (1, model::PREVIOUS_DIGEST.to_owned())
        );
        assert!(sqlx::query_scalar::<_,bool>("SELECT to_regclass('public.core_initial_history') IS NULL AND NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='public' AND table_name='media_assets' AND column_name='owner_kind')").fetch_one(&mut owner).await.unwrap());
        let mut second = PgConnection::connect_with(&options.clone().username("core_initialize"))
            .await
            .unwrap();
        let (a, b) = tokio::join!(upgrade(&mut owner), upgrade(&mut second));
        assert_eq!(usize::from(a.unwrap()) + usize::from(b.unwrap()), 1);
        let history: serde_json::Value =
            sqlx::query_scalar("SELECT to_jsonb(h) FROM public.core_initial_history h")
                .fetch_one(&mut owner)
                .await
                .unwrap();
        assert!(!upgrade(&mut owner).await.unwrap());
        assert!(!crate::initialize(&mut owner).await.unwrap());
        let replay: serde_json::Value =
            sqlx::query_scalar("SELECT to_jsonb(h) FROM public.core_initial_history h")
                .fetch_one(&mut owner)
                .await
                .unwrap();
        assert_eq!(history, replay);
        let retained:serde_json::Value=sqlx::query_scalar("SELECT to_jsonb(a)-'owner_kind'-'purpose' FROM public.media_assets a WHERE file_id='preserved'").fetch_one(&mut owner).await.unwrap();
        assert_eq!(retained, original);
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT owner_kind||'|'||purpose||'|'||owner_id FROM public.media_assets WHERE file_id='preserved'").fetch_one(&mut owner).await.unwrap(),"user|general|actual-user");
        assert!(
            !sqlx::query_scalar::<_, bool>(
                "SELECT global_enabled FROM public.capability_service_settings WHERE id=1"
            )
            .fetch_one(&mut owner)
            .await
            .unwrap()
        );
        let mut runtime = PgConnection::connect_with(&options.username("core_runtime"))
            .await
            .unwrap();
        assert!(upgrade(&mut runtime).await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM public.core_initial_history")
                .fetch_one(&mut runtime)
                .await
                .unwrap(),
            1
        );
        assert!(
            sqlx::query("UPDATE public.core_initial_history SET applied_at=clock_timestamp()")
                .execute(&mut runtime)
                .await
                .is_err()
        );
        assert!(sqlx::query("INSERT INTO public.media_assets(tenant_id,file_id,owner_id,file_name,mime_type,file_size,url,owner_kind,purpose) VALUES('fixture','bad','other','f','image/png',1,'','application','avatar')").execute(&mut runtime).await.is_err());
        sqlx::query("UPDATE public.core_initial_history SET ddl_sha256=repeat('0',64)")
            .execute(&mut owner)
            .await
            .unwrap();
        assert!(upgrade(&mut owner).await.is_err());
        assert!(crate::initialize(&mut owner).await.is_err());
        runtime.close().await.unwrap();
        second.close().await.unwrap();
        owner.close().await.unwrap();
        admin.close().await.unwrap();
    }
}
