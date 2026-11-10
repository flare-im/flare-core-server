//! `CapabilityService` 与 `Administer` 子命令分发。

mod administer;
#[cfg(test)]
mod hook_admin_pg_tests;
#[cfg(test)]
mod hook_executions_pg_tests;
mod plugin_registry;
mod service;
#[cfg(test)]
mod source_audit_pg_tests;

pub use service::{CapabilityGrpcServer, CapabilityInvocationMetrics, CapabilityMetricsSnapshot};
