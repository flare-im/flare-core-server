//! **Query**：集成形态目录（无副作用，供控制面 / 运维 / UI 展示）。

use crate::domain::hook_integration::HookTransportSurface;

/// 单条对外说明（稳定 slug，便于与配置 JSON 对齐）。
#[derive(Debug, Clone)]
pub struct HookIntegrationChannelDoc {
    pub surface: HookTransportSurface,
    pub slug: &'static str,
    pub summary: &'static str,
}

/// 返回当前版本支持的两种出站目标说明。
#[must_use]
pub fn list_hook_integration_channels() -> Vec<HookIntegrationChannelDoc> {
    vec![
        HookIntegrationChannelDoc {
            surface: HookTransportSurface::GrpcHook,
            slug: "grpc",
            summary: "进程外 gRPC（HookPlugin.Call）；支持 endpoint 直连或 service_name + 服务发现。",
        },
        HookIntegrationChannelDoc {
            surface: HookTransportSurface::WebhookHook,
            slug: "webhook",
            summary: "HTTP Webhook；JSON 载荷与签名头见 Hook 合同。",
        },
    ]
}
