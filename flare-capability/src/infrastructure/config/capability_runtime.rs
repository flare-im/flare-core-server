//! `CapabilityService` 生产运行时参数（环境变量，与网关 / 编排协同）。

use std::path::Path;
use std::time::Duration;

use flare_core_base::config::LayeredConfig;

/// 能力 gRPC 的运行时护栏：负载、超时、滥用防护。
#[derive(Debug, Clone)]
pub struct CapabilityRuntimeConfig {
    /// `Dispatch.payload_json` 最大字节数（UTF-8）。
    pub max_payload_json_bytes: usize,
    /// 单次 `Dispatch` 端到端超时。
    pub dispatch_timeout: Duration,
    /// 每 `(tenant_id, user_id)` 每分钟最多 `Dispatch` 次数；`None` 表示不限制。
    pub dispatch_max_per_minute: Option<u32>,
    /// 插件远程调用超时。
    pub plugin_call_timeout: Duration,
}

impl CapabilityRuntimeConfig {
    /// 配置来源优先级：环境变量 > 配置文件(`config_file`) > 默认值。
    pub fn from_sources(config_file: Option<&Path>) -> Self {
        let layered = LayeredConfig::from_optional_toml(config_file);

        let mut max_payload_json_bytes = 1_048_576usize;
        let mut dispatch_timeout_ms = 25_000u64;
        let mut dispatch_max_per_minute: Option<u32> = None;
        let mut plugin_call_timeout_ms = 5_000u64;

        if let Some(v) = layered
            .resolve_usize(
                "FLARE_CAPABILITY_MAX_PAYLOAD_BYTES",
                "capability_runtime.max_payload_json_bytes",
            )
            .filter(|&n: &usize| n > 0 && n <= 16 * 1024 * 1024)
        {
            max_payload_json_bytes = v;
        }

        if let Some(v) = layered
            .resolve_u64(
                "FLARE_CAPABILITY_DISPATCH_TIMEOUT_MS",
                "capability_runtime.dispatch_timeout_ms",
            )
            .filter(|&n: &u64| (1_000..=120_000).contains(&n))
        {
            dispatch_timeout_ms = v;
        }

        if let Some(v) = layered
            .resolve_u32(
                "FLARE_CAPABILITY_DISPATCH_MAX_PER_MINUTE",
                "capability_runtime.dispatch_max_per_minute",
            )
            .filter(|&n: &u32| n > 0 && n <= 100_000)
        {
            dispatch_max_per_minute = Some(v);
        }

        if let Some(v) = layered
            .resolve_u64(
                "FLARE_CAPABILITY_PLUGIN_CALL_TIMEOUT_MS",
                "capability_runtime.plugin_call_timeout_ms",
            )
            .filter(|&n: &u64| (200..=60_000).contains(&n))
        {
            plugin_call_timeout_ms = v;
        }

        let cfg = Self {
            max_payload_json_bytes,
            dispatch_timeout: Duration::from_millis(dispatch_timeout_ms),
            dispatch_max_per_minute,
            plugin_call_timeout: Duration::from_millis(plugin_call_timeout_ms),
        };

        tracing::info!(
            max_payload_bytes = cfg.max_payload_json_bytes,
            dispatch_timeout_ms = dispatch_timeout_ms,
            dispatch_rate_per_minute = ?cfg.dispatch_max_per_minute,
            plugin_call_timeout_ms = plugin_call_timeout_ms,
            "CapabilityRuntimeConfig loaded"
        );

        cfg
    }

    /// 无配置文件场景（仅 env + 默认值）。
    pub fn from_env() -> Self {
        Self::from_sources(None)
    }
}
