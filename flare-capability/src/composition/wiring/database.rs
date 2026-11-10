//! **数据库装配**：连接池与策略表的启动检查。

use std::sync::Arc;

use flare_server_core::error::{AnyhowContext, Result};

use crate::composition::process_config::CapabilityServiceConfig;
use crate::infrastructure::persistence::postgres_config::PostgresHookConfigRepository;

/// Hook 配置持久化仓储（仅当配置了 `database_url`）。
pub(crate) type HookConfigRepository = Arc<PostgresHookConfigRepository>;

pub(crate) fn log_capability_db_target(database_url: &str) {
    let tail = database_url
        .strip_prefix("postgresql://")
        .or_else(|| database_url.strip_prefix("postgres://"))
        .and_then(|rest| rest.find('@').map(|i| &rest[i + 1..]));
    match tail {
        Some(host_db) => {
            tracing::info!(
                target = "flare_capability::db",
                postgres_after_at = %host_db,
                "flare-capability PostgreSQL target (credentials omitted); deploy/init.sql must be applied to this host/database"
            );
        }
        None => {
            tracing::warn!(
                target = "flare_capability::db",
                "could not parse database_url for logging"
            );
        }
    }
}

/// 打开 Core 库（配了 `database_url` 时），并确认策略表在这个库里。
pub(crate) async fn open_database(
    config: &CapabilityServiceConfig,
) -> Result<Option<HookConfigRepository>> {
    let Some(database_url) = config.database_url.as_ref() else {
        return Ok(None);
    };
    let repository = Arc::new(
        PostgresHookConfigRepository::new_with_pool_profile(
            database_url,
            config.postgres_max_connections,
            config.postgres_min_connections,
            config.postgres_acquire_timeout_seconds,
            config.postgres_idle_timeout_seconds,
            config.postgres_max_lifetime_seconds,
        )
        .await
        .context("Failed to create database config repository")?,
    );

    log_capability_db_target(database_url);
    crate::infrastructure::persistence::PostgresCapabilityPolicy::assert_public_capability_schema(
        repository.connection_pool().as_ref(),
    )
    .await
    .context(
        "Capability policy PostgreSQL schema (public.capability_*) missing or wrong database",
    )?;
    Ok(Some(repository))
}
