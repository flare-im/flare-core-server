//! `hook_executions` 保留期的参数：环境变量 > 运行时配置文件（`[hook_executions]`）> 默认值。超出范围
//! 的值被忽略、用默认值（与 `capability_runtime` 相同）。写入与熔断的参数在 Ingest。

use std::path::Path;
use std::time::Duration;

use flare_core_base::config::LayeredConfig;

use crate::infrastructure::hook_retention::HookRetentionTask;

/// 保留期清理的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRetentionConfig {
    /// 关掉时不清理。
    pub enabled: bool,
    pub retention: Duration,
    pub sweep_interval: Duration,
    pub delete_batch: u32,
}

impl Default for HookRetentionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention: Duration::from_secs(7 * 86_400),
            sweep_interval: Duration::from_secs(600),
            delete_batch: 5_000,
        }
    }
}

impl HookRetentionConfig {
    pub fn task(&self) -> HookRetentionTask {
        HookRetentionTask {
            retention: self.retention,
            sweep_interval: self.sweep_interval,
            delete_batch: self.delete_batch,
        }
    }

    pub fn from_sources(config_file: Option<&Path>) -> Self {
        let settings = Self::from_layered(&LayeredConfig::from_optional_toml(config_file));
        tracing::info!(
            enabled = settings.enabled,
            retention_days = settings.retention.as_secs() / 86_400,
            sweep_interval_secs = settings.sweep_interval.as_secs(),
            delete_batch = settings.delete_batch,
            "hook execution retention settings loaded"
        );
        settings
    }

    fn from_layered(layered: &LayeredConfig) -> Self {
        let defaults = Self::default();
        Self {
            enabled: layered
                .resolve_bool(
                    "FLARE_CAPABILITY_HOOK_EXECUTIONS_RETENTION_ENABLED",
                    "hook_executions.retention_enabled",
                )
                .unwrap_or(defaults.enabled),
            retention: layered
                .resolve_u64(
                    "FLARE_CAPABILITY_HOOK_EXECUTIONS_RETENTION_DAYS",
                    "hook_executions.retention_days",
                )
                .filter(|n| (1..=90).contains(n))
                .map(|days| Duration::from_secs(days * 86_400))
                .unwrap_or(defaults.retention),
            sweep_interval: layered
                .resolve_u64(
                    "FLARE_CAPABILITY_HOOK_EXECUTIONS_RETENTION_SWEEP_SECS",
                    "hook_executions.retention_sweep_secs",
                )
                .filter(|n| (10..=86_400).contains(n))
                .map(Duration::from_secs)
                .unwrap_or(defaults.sweep_interval),
            delete_batch: layered
                .resolve_u32(
                    "FLARE_CAPABILITY_HOOK_EXECUTIONS_RETENTION_DELETE_BATCH",
                    "hook_executions.retention_delete_batch",
                )
                .filter(|n| (100..=100_000).contains(n))
                .unwrap_or(defaults.delete_batch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_toml(raw: &str) -> HookRetentionConfig {
        let dir = std::env::temp_dir().join(format!(
            "flare-capability-hook-retention-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("capability.toml");
        std::fs::write(&path, raw).unwrap();
        let settings =
            HookRetentionConfig::from_layered(&LayeredConfig::from_optional_toml(Some(&path)));
        std::fs::remove_dir_all(&dir).unwrap();
        settings
    }

    #[test]
    fn defaults_apply_without_a_file() {
        assert_eq!(
            HookRetentionConfig::from_layered(&LayeredConfig::default()),
            HookRetentionConfig::default()
        );
    }

    #[test]
    fn file_values_are_read_and_out_of_range_values_fall_back_to_defaults() {
        let settings = from_toml(
            r#"
[hook_executions]
retention_enabled = false
retention_days = 3
retention_sweep_secs = 60
retention_delete_batch = 1000
"#,
        );
        assert!(!settings.enabled);
        assert_eq!(settings.retention, Duration::from_secs(3 * 86_400));
        assert_eq!(settings.sweep_interval, Duration::from_secs(60));
        assert_eq!(settings.delete_batch, 1_000);

        let settings = from_toml("[hook_executions]\nretention_days = 0\n");
        assert_eq!(settings.retention, HookRetentionConfig::default().retention);
    }
}
