//! 进程 **组合根**（Composition Root）：按 **DDD + CQRS** 拆分为配置 DTO、运行时上下文、**wiring** 子模块与启动器。
//!
//! - **`process_config`**：进程启动参数（非领域模型）。
//! - **`runtime_context`**：装配结果句柄（供接口层使用，对应「应用运行时」读/写侧入口）。
//! - **`wiring`**：依赖图构建（数据库 → Hook 治理 → 能力扩展栈）；**`initialize`** 为唯一总装入口。
//! - **`bootstrap`**：监听地址解析 + gRPC `Server` + `ServiceRuntime`（基础设施边界）。
//! - **`extensions`**：二进制在启动时注入的可选能力（插件服务路由）；开源构建不注入。
//!
//! 此处 **不实现** 业务规则，只做接线。

pub mod bootstrap;
pub mod extensions;
pub mod process_config;
pub mod runtime_context;
pub mod wiring;

pub use bootstrap::ApplicationBootstrap;
pub use extensions::{CapabilityExtensions, ServiceDispatchFactory, ServiceDispatchParts};
pub use process_config::CapabilityServiceConfig;
pub use runtime_context::ApplicationContext;
pub use wiring::init_capability_extension_stack;
