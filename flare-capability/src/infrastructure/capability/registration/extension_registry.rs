//! 能力扩展注册表（基础设施侧统一容器）。

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::domain::capability::{
    CapabilityDispatchRoute, DynExtensionOperationHandler, RecipientResolver,
};
use crate::interface::grpc::ExtensionPluginRouter;

use crate::infrastructure::capability::dispatch::RecipientResolverRuntime;

pub struct CapabilityExtensionRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

#[derive(Default)]
pub struct RegistryInner {
    pub recipient: RecipientResolverRuntime,
    pub extension_router: ExtensionPluginRouter,
    /// 分发路由表：**核心不认识任何具体插件**，只按注册顺序询问谁接管。
    /// 由组合根装配，空表时全部走远端插件路由。
    pub dispatch_routes: Vec<Arc<dyn CapabilityDispatchRoute>>,
}

impl CapabilityExtensionRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner::default())),
        }
    }

    pub async fn recipient(&self) -> RecipientResolverRuntime {
        self.inner.read().await.recipient.clone()
    }

    /// 注册一条分发路由。**顺序即优先级**：先注册的先被询问。
    pub async fn register_dispatch_route(&self, route: Arc<dyn CapabilityDispatchRoute>) {
        self.inner.write().await.dispatch_routes.push(route);
    }

    pub async fn dispatch_routes(&self) -> Vec<Arc<dyn CapabilityDispatchRoute>> {
        self.inner.read().await.dispatch_routes.clone()
    }

    pub async fn register_recipient_resolver(&self, resolver: Arc<dyn RecipientResolver>) {
        self.inner.read().await.recipient.register(resolver).await;
    }

    /// 取通用 `ExtensionPlugin` 路由器（core 与 binary 装 tonic service 时共享同一实例）。
    pub async fn extension_router(&self) -> ExtensionPluginRouter {
        self.inner.read().await.extension_router.clone()
    }

    /// 由进程内扩展在 `wire(..)` 时注册自己的 operation handler。
    pub async fn register_extension_operations(&self, handler: DynExtensionOperationHandler) {
        self.inner
            .read()
            .await
            .extension_router
            .register(handler)
            .await;
    }
}

impl Default for CapabilityExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CapabilityExtensionRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
