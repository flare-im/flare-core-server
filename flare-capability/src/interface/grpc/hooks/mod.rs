//! Hook 治理：`CapabilityService.Administer` 背后的配置、统计与执行记录。

mod hook_service;
#[cfg(test)]
pub(crate) mod test_support;

pub use hook_service::HookServiceServer;
