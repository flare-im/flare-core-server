//! 能力扩展基础设施（按职责分包）。
//!
//! - [`registration`]：扩展注册表与授权内存模型
//! - [`dispatch`]：Recipient 运行时与 Dispatch 限流
//! - [`routing`]：插件路由簿
//! - [`adapters`] / [`builtin`]：可选适配器与内置 Resolver

pub mod adapters;
pub mod builtin;
pub mod dispatch;
pub mod registration;
pub mod routing;

pub(crate) mod plugin_channel;

pub use dispatch::{DispatchRateLimiter, RecipientResolverRuntime};
pub use registration::{CapabilityExtensionRegistry, InMemoryCapabilityGrants, RegistryInner};
pub use routing::PluginRouteBook;
