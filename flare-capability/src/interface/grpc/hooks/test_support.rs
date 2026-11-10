//! 测试用：Cell 签名密钥与 Hook 配置样例。

use std::collections::HashMap;

use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
use flare_server_core::context::Ctx;
use flare_server_core::grpc::middleware::signed_context::verify_signed_context_metadata;
use flare_server_core::{
    SignedActorType, SignedContextIdentity, SignedContextSigner, SignedContextVerifier,
};

use crate::domain::model::{HookConfigItem, HookSelectorConfig, HookTransportConfig};

/// Cell 内部上下文的签名与验签密钥（同一对）。
pub(crate) struct CellKeys {
    pub signer: SignedContextSigner,
    pub verifier: SignedContextVerifier,
}

pub(crate) fn cell_keys(seed: u8) -> CellKeys {
    let key = SigningKey::from_bytes(&[seed; 32]);
    let private = key.to_pkcs8_pem(Default::default()).unwrap();
    let public = key
        .verifying_key()
        .to_public_key_pem(Default::default())
        .unwrap();
    CellKeys {
        signer: SignedContextSigner::from_private_key(&private).unwrap(),
        verifier: SignedContextVerifier::from_public_key(&public).unwrap(),
    }
}

/// 验过签的调用方上下文（带原始令牌）。
pub(crate) fn signed_ctx(keys: &CellKeys, actor: SignedActorType, id: &str, tenant: &str) -> Ctx {
    let request_id = format!("req-{}", uuid::Uuid::new_v4().simple());
    let token = keys
        .signer
        .sign(&SignedContextIdentity::new(actor, id, tenant, &request_id))
        .unwrap();
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert("x-flare-ctx", token.parse().unwrap());
    verify_signed_context_metadata(&metadata, &keys.verifier).unwrap()
}

/// 一条 PreSend Webhook 配置，`secret` 为签名密钥。
pub(crate) fn signed_webhook_hook(
    name: &str,
    endpoint: &str,
    secret: Option<&str>,
) -> HookConfigItem {
    HookConfigItem {
        name: name.into(),
        description: None,
        enabled: true,
        order: 900,
        timeout_ms: 5_000,
        require_success: true,
        selector: HookSelectorConfig::default(),
        transport: HookTransportConfig::Webhook {
            endpoint: endpoint.into(),
            secret: secret.map(str::to_string),
            headers: HashMap::new(),
        },
        metadata: HashMap::new(),
    }
}
