//! 领域层：多限界上下文
//!
//! - **Hook 控制面**：[`model`]（配置与执行证据的查询模型）、[`repository`]
//! - **HookIntegration**：[`hook_integration`]（目标分类与写入校验）
//! - **Capability**：[`capability`]（Resolver、分发 DTO、策略端口）
//! - **插件注册表**：[`plugin`]（版本、部署、应用状态）

pub mod capability;
pub mod hook_integration;
pub mod model;
pub mod plugin;
pub mod repository;

pub use model::*;
pub use repository::{
    HookExecutionRepository, PluginAuthorizationReader, PluginEventDirectory,
    TenantProjectionRepository, TenantRuntimeReader, entity_lifecycle, plugin_card,
    plugin_registry, plugin_runtime, plugin_state, realtime_delivery, resource_access,
    service_dispatch, tenant_notice_outbox,
};
