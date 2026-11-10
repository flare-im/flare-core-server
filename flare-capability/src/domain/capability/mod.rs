//! 能力扩展限界上下文：Resolver、分发命令、策略端口。

pub mod context;
pub mod descriptor;
pub mod dispatch;
pub mod error;
pub mod extension_operation;
pub mod grant;
pub mod ports;
pub mod recipient;

pub use context::{CapabilityInvokeMeta, ConversationKind, ResolveTrigger};
pub use descriptor::CapabilityDescriptor;
pub use dispatch::{
    CapabilityDispatchCommand, CapabilityDispatchResult, DISPATCH_TENANT_REQUIRED,
    require_dispatch_tenant, trimmed_dispatch_tenant,
};
pub use error::{CapabilityError, Result};
pub use extension_operation::{DynExtensionOperationHandler, ExtensionOperationHandler};
pub use grant::TenantCapabilitySwitch;
pub use ports::{CapabilityDispatchRoute, CapabilityPolicyBackend, RecipientResolver};
pub use recipient::{RecipientResolveRequest, RecipientResolveResult};
