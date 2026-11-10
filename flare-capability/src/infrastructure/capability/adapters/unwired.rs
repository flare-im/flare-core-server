//! 未接线的占位适配器（单一实现，避免按传输层重复一份空壳）。

use async_trait::async_trait;
use flare_core_base::context::Ctx;

use crate::domain::capability::{
    CapabilityError, ConversationKind, RecipientResolveRequest, RecipientResolveResult,
    RecipientResolver, ResolveTrigger, Result,
};

/// 接收者解析占位（未启用任何远端解析器）
pub struct UnwiredRecipientResolver;

impl Default for UnwiredRecipientResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl UnwiredRecipientResolver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl RecipientResolver for UnwiredRecipientResolver {
    fn id(&self) -> &str {
        "unwired.recipient"
    }

    fn supports(&self, _kind: &ConversationKind, _trigger: ResolveTrigger) -> bool {
        false
    }

    async fn resolve(
        &self,
        _ctx: &Ctx,
        _req: &RecipientResolveRequest,
    ) -> Result<RecipientResolveResult> {
        Err(CapabilityError::NotSupported(
            "UnwiredRecipientResolver".into(),
        ))
    }
}
