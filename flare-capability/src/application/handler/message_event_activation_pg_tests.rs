//! The durable grant boundary, exercised in a temporary schema and rolled back.
use sqlx::{Connection, Row};

#[tokio::test]
#[ignore = "requires FLARE_PLUGIN_EVENT_TEST_DATABASE_URL; only temporary objects are created"]
async fn event_activation_survives_renewals_and_advances_on_new_body_grants_and_configuration() {
    let url = std::env::var("FLARE_PLUGIN_EVENT_TEST_DATABASE_URL")
        .expect("explicit event test PostgreSQL URL");
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    let mut tx = connection.begin().await.unwrap();
    let schema = format!("plugin_event_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema}; SET LOCAL search_path={schema},pg_temp;"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::raw_sql("CREATE TABLE plugin_app_states(
        app_id text,binding_id text,runtime_revision bigint DEFAULT 1,runtime_configuration_digest text,runtime_healthy_until_ms bigint DEFAULT 0,
        approved jsonb DEFAULT '{\"dataScopes\":[\"messages.body.read\"]}',features jsonb DEFAULT '{}',configuration jsonb DEFAULT '{}',
        lifecycle text DEFAULT 'installed',enabled boolean DEFAULT true,suspended boolean DEFAULT false,
        owned boolean DEFAULT true,offline_hold boolean DEFAULT false,manifest_digest text DEFAULT 'manifest',
        projected_revision bigint DEFAULT 1,updated_at bigint DEFAULT 1);
        INSERT INTO plugin_app_states(app_id,binding_id) VALUES('app','grant');")
        .execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../migrations/0028_20261108_plugin_event_activation.sql"
    )))
    .execute(&mut *tx)
    .await
    .unwrap();
    let first: i64 = sqlx::query_scalar("SELECT event_active_since_ms FROM plugin_app_states")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(first > 0);
    sqlx::query("UPDATE plugin_app_states SET projected_revision=2,runtime_revision=2,runtime_healthy_until_ms=99999,updated_at=99")
        .execute(&mut *tx).await.unwrap();
    let renewed: i64 = sqlx::query_scalar("SELECT event_active_since_ms FROM plugin_app_states")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        first, renewed,
        "lease/runtime renewal must preserve eligible backlog"
    );
    let mut previous = renewed;
    for update in [
        "UPDATE plugin_app_states SET approved='{}'",
        "UPDATE plugin_app_states SET approved='{\"dataScopes\":[\"messages.body.read\"]}'",
        "UPDATE plugin_app_states SET configuration='{\"keyword\":\"checkin\"}'",
        "UPDATE plugin_app_states SET features='{\"daily\":false}'",
        "UPDATE plugin_app_states SET enabled=false",
        "UPDATE plugin_app_states SET enabled=true",
    ] {
        sqlx::query(update).execute(&mut *tx).await.unwrap();
        let current: i64 = sqlx::query("SELECT event_active_since_ms FROM plugin_app_states")
            .fetch_one(&mut *tx)
            .await
            .unwrap()
            .get(0);
        assert!(
            current > previous,
            "a new effective grant/configuration rejects earlier messages: {update}"
        );
        previous = current;
    }
    sqlx::query("UPDATE plugin_app_states SET configuration=configuration,features=features,event_active_since_ms=1")
        .execute(&mut *tx).await.unwrap();
    let same: i64 = sqlx::query_scalar("SELECT event_active_since_ms FROM plugin_app_states")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        previous, same,
        "idempotent writes cannot rewind or advance the boundary"
    );
    tx.rollback().await.unwrap();
}
