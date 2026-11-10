//! **Hook 治理装配**：配置写入、统计与执行记录查询、保留期清理。

use std::sync::Arc;

use crate::application::handler::HookQueryHandler;
use crate::composition::wiring::database::HookConfigRepository;
use crate::domain::repository::HookExecutionRepository;
use crate::infrastructure::config::HookRetentionConfig;
use crate::infrastructure::hook_retention::{HookRetentionMetrics, HookRetentionTask};
use crate::infrastructure::persistence::PostgresHookExecutionRepository;
use crate::interface::grpc::HookServiceServer;

/// 保留期清理所需（有库且开启时）：由启动器挂到进程运行时上。
pub(crate) struct HookRetentionBackground {
    pub repository: Arc<dyn HookExecutionRepository>,
    pub metrics: Arc<HookRetentionMetrics>,
    pub task: HookRetentionTask,
}

/// Hook 侧已装配的治理服务。
pub(crate) struct HookGovernanceReady {
    pub hook_governance: Option<Arc<HookServiceServer>>,
    pub retention: Option<HookRetentionBackground>,
}

pub(crate) fn build_hook_governance(
    repository: Option<&HookConfigRepository>,
    settings: &HookRetentionConfig,
) -> HookGovernanceReady {
    let Some(repository) = repository else {
        tracing::warn!("database_url not set: Hook governance (Administer) unavailable");
        return HookGovernanceReady {
            hook_governance: None,
            retention: None,
        };
    };
    let executions: Arc<dyn HookExecutionRepository> = Arc::new(
        PostgresHookExecutionRepository::new(repository.connection_pool()),
    );
    let governance = HookServiceServer::new(repository.clone())
        .with_executions(Arc::new(HookQueryHandler::new(Arc::clone(&executions))));
    let retention = settings.enabled.then(|| HookRetentionBackground {
        repository: executions,
        metrics: HookRetentionMetrics::registered(),
        task: settings.task(),
    });
    HookGovernanceReady {
        hook_governance: Some(Arc::new(governance)),
        retention,
    }
}
