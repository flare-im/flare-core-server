//! Runtime convergence and the keys of plugin configuration secrets.
use std::sync::Arc;

use flare_server_core::error::Result;
use sqlx::PgPool;

use crate::application::handler::plugin_reconcile::{PluginReconciler, ReconcileSignal};
use crate::domain::repository::plugin_registry::PluginRegistryRepository;
use crate::infrastructure::adapters::plugin_runtime::GrpcPluginRuntime;
use crate::infrastructure::capability::PluginRouteBook;
use crate::infrastructure::config::plugin_config_key::PluginConfigCipher;

/// The keyring, refusing to start when the database holds secrets it cannot open.
pub(super) async fn config_cipher(pool: &PgPool) -> Result<PluginConfigCipher> {
    let cipher = PluginConfigCipher::from_env()?;
    cipher.check_sealed(pool).await?;
    Ok(cipher)
}

/// Delivery is signed with the plugin call key. Without it nothing converges, and no plugin call
/// could run either.
pub(super) fn reconciler(
    repository: Arc<dyn PluginRegistryRepository>,
    routes: &PluginRouteBook,
    development: bool,
) -> Option<Arc<PluginReconciler>> {
    let Some(signer) = routes.invocation_signer.clone() else {
        tracing::warn!("plugin invocation signer absent; plugin runtime convergence is off");
        return None;
    };
    Some(Arc::new(PluginReconciler::new(
        repository,
        Arc::new(GrpcPluginRuntime),
        signer,
        Arc::new(ReconcileSignal::default()),
        development,
    )))
}
