//! Hook **出站目标** 在领域层的分类（与 [`crate::domain::model::HookTransportConfig`] 一一对应）。

use crate::domain::model::HookTransportConfig;

/// 目标的语义（供查询/审计/文档化；不承载网络细节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookTransportSurface {
    /// 进程外 gRPC（直连 endpoint 或经注册中心解析 `service_name`）。
    GrpcHook,
    /// HTTP Webhook（同步请求/响应）。
    WebhookHook,
}

#[must_use]
pub fn classify_transport(config: &HookTransportConfig) -> HookTransportSurface {
    match config {
        HookTransportConfig::Grpc { .. } => HookTransportSurface::GrpcHook,
        HookTransportConfig::Webhook { .. } => HookTransportSurface::WebhookHook,
    }
}
