//! 能力扩展的基础设施：外部适配器、Hook 执行证据的保留期清理、PostgreSQL 持久化。

pub mod adapters;
pub mod capability;
pub mod config;
pub mod hook_retention;
pub mod persistence;
