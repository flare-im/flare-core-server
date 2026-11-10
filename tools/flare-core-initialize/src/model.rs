//! Source-bound initial schema admission and authority checks.
use crate::{SEEDS, SQL, digest, ensure};
use flare_server_core::error::{FlareError, Result};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
pub const PREVIOUS_SQL: &str = include_str!("../../../initial/versions/v1/schema.sql");
pub const PREVIOUS_SEEDS: &str = include_str!("../../../initial/versions/v1/seeds.sql");
pub const PREVIOUS_DIGEST: &str =
    "0ef150419f08a05d966226025d426e7cb2cf6013d976565f0002e9b733275093";
pub const UPGRADE_SQL: &str = include_str!("../../../initial/upgrades/0002_application_media.sql");
const CATALOG: &str = include_str!("../../../initial/upgrades/catalog.sql");
const ROLES: &str = include_str!("../../../initial/roles.sql");
const DATABASE: &str = include_str!("../../../initial/database.sql");
pub fn sha(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub fn previous_digest() -> String {
    let mut h = Sha256::new();
    h.update(PREVIOUS_SQL);
    h.update(b"\0");
    h.update(PREVIOUS_SEEDS);
    format!("{:x}", h.finalize())
}
pub async fn begin_checks(connection: &mut PgConnection) -> Result<()> {
    ensure(
        previous_digest() == PREVIOUS_DIGEST,
        "Core accepted v1 source digest differs",
    )?;
    sqlx::raw_sql("SET LOCAL search_path=pg_catalog,public,pg_temp; SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='30s'; SELECT pg_advisory_xact_lock(5065494824365673545);")
        .execute(&mut *connection).await.map_err(|_|FlareError::system("Core initializer is busy"))?;
    let allowed:bool=sqlx::query_scalar("SELECT current_user=session_user AND current_user='core_initialize' AND d.datdba=r.oid AND NOT EXISTS(SELECT 1 FROM pg_roles p WHERE p.rolname IN ('core_initialize','core_runtime') AND (p.rolsuper OR p.rolbypassrls OR p.rolcreaterole OR p.rolcreatedb OR p.rolreplication OR NOT p.rolcanlogin OR (p.rolname='core_runtime' AND has_database_privilege(p.oid,current_database(),'CREATE')) OR EXISTS(SELECT 1 FROM pg_auth_members m WHERE m.member=p.oid))) AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname='core_runtime') FROM pg_roles r CROSS JOIN pg_database d WHERE r.rolname=current_user AND d.datname=current_database()")
        .fetch_one(&mut *connection).await.map_err(|_|FlareError::system("Core initializer role check unavailable"))?;
    ensure(
        allowed,
        "Core initializer requires its isolated database-owner login and runtime role",
    )?;
    let extension:bool=sqlx::query_scalar("SELECT current_setting('server_version_num')::int/10000=16 AND EXISTS(SELECT 1 FROM pg_extension e JOIN pg_roles r ON r.oid=e.extowner WHERE e.extname='timescaledb' AND e.extversion IN ('2.29.1','2.30.0') AND r.rolsuper) AND EXISTS(SELECT 1 FROM pg_extension e JOIN pg_roles r ON r.oid=e.extowner JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='plpgsql' AND n.nspname='pg_catalog' AND r.rolsuper) AND NOT EXISTS(SELECT 1 FROM pg_extension WHERE extname NOT IN ('timescaledb','plpgsql')) AND to_regclass('public._sqlx_migrations') IS NULL AND to_regclass('public.flare_schema_migrations') IS NULL")
        .fetch_one(&mut *connection).await.map_err(|_|FlareError::system("Core initial model check unavailable"))?;
    ensure(
        extension,
        "Core initial model requires reviewed PostgreSQL 16/TimescaleDB and no SQLx migration ledger",
    )
}
pub async fn lock_tables(connection: &mut PgConnection) -> Result<()> {
    let tables:Vec<String>=sqlx::query_scalar("SELECT quote_ident(n.nspname)||'.'||quote_ident(c.relname) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p') AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='e') ORDER BY c.relname")
        .fetch_all(&mut *connection).await.map_err(|_|FlareError::system("Core initial table inventory unavailable"))?;
    for table in tables {
        sqlx::raw_sql(&format!("LOCK TABLE {table} IN SHARE ROW EXCLUSIVE MODE"))
            .execute(&mut *connection)
            .await
            .map_err(|_| FlareError::system("Core initial tables are busy"))?;
    }
    Ok(())
}
pub async fn shape(connection: &mut PgConnection, version: i16) -> Result<()> {
    let ts: String =
        sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname='timescaledb'")
            .fetch_one(&mut *connection)
            .await
            .map_err(|_| FlareError::system("Core TimescaleDB version unavailable"))?;
    let text = match (version, ts.as_str()) {
        (1, "2.29.1") => include_str!("../../../initial/upgrades/v1-pg16-ts2.29.1.json"),
        (1, "2.30.0") => include_str!("../../../initial/upgrades/v1-pg16-ts2.30.0.json"),
        (2, "2.29.1") => include_str!("../../../initial/upgrades/v2-pg16-ts2.29.1.json"),
        (2, "2.30.0") => include_str!("../../../initial/upgrades/v2-pg16-ts2.30.0.json"),
        _ => {
            return Err(FlareError::system(
                "Core initial descriptor environment is not reviewed",
            ));
        }
    };
    let expected: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| FlareError::system("Core initial descriptor is invalid"))?;
    for (name, value) in [
        ("initial/versions/v1/schema.sql", PREVIOUS_SQL),
        ("initial/versions/v1/seeds.sql", PREVIOUS_SEEDS),
        ("initial/schema.sql", SQL),
        ("initial/seeds.sql", SEEDS),
        ("initial/roles.sql", ROLES),
        ("initial/database.sql", DATABASE),
        ("initial/upgrades/0002_application_media.sql", UPGRADE_SQL),
        ("initial/upgrades/catalog.sql", CATALOG),
    ] {
        ensure(
            expected["sourceSha256"][name].as_str() == Some(sha(value).as_str()),
            "Core initial descriptor source drift",
        )?;
    }
    ensure(
        expected["bundleSha256"].as_str()
            == Some(
                if version == 1 {
                    PREVIOUS_DIGEST.to_owned()
                } else {
                    digest()
                }
                .as_str(),
            ),
        "Core initial descriptor bundle drift",
    )?;
    let actual: serde_json::Value = sqlx::query_scalar(CATALOG)
        .fetch_one(connection)
        .await
        .map_err(|_| FlareError::system("Core initial catalog unavailable"))?;
    ensure(
        actual == expected["objects"],
        "Core initial schema shape, owner, ACL or Timescale settings differ from the accepted source model",
    )
}
pub async fn marker(connection: &mut PgConnection) -> Result<(i16, String)> {
    let rows: Vec<(i16, String)> =
        sqlx::query_as("SELECT version,schema_sha256 FROM public.core_initial_schema FOR UPDATE")
            .fetch_all(connection)
            .await
            .map_err(|_| FlareError::system("Core initial marker unavailable"))?;
    ensure(
        rows.len() == 1,
        "Core initial marker must contain exactly one version",
    )?;
    Ok(rows.into_iter().next().expect("checked singleton"))
}
pub async fn verify_current(connection: &mut PgConnection) -> Result<()> {
    ensure(
        marker(connection).await? == (2, digest()),
        "Core initial version differs; accepted v1 requires the explicit upgrade command",
    )?;
    shape(connection, 2).await?;
    let history:Vec<(i16,i16,Option<String>,String,String,String)>=sqlx::query_as("SELECT to_version,from_version,from_sha256,to_sha256,transition,ddl_sha256 FROM public.core_initial_history").fetch_all(connection).await.map_err(|_|FlareError::system("Core initial history unavailable"))?;
    ensure(
        history.len() == 1,
        "Core initial history must contain its genuine transition",
    )?;
    let (to, from, old, target, transition, ddl) = &history[0];
    ensure(
        *to == 2
            && *target == digest()
            && ((*from == 0 && old.is_none() && transition == "fresh" && *ddl == digest())
                || (*from == 1
                    && old.as_deref() == Some(PREVIOUS_DIGEST)
                    && transition == "application-media-v2"
                    && *ddl == sha(UPGRADE_SQL))),
        "Core initial history provenance differs",
    )
}
