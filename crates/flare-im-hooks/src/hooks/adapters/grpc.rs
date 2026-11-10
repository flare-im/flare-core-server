use std::collections::HashMap;
use std::sync::Arc;

use crate::Ctx;
use async_trait::async_trait;
use flare_grpc_proto::capability::hook_plugin_client::HookPluginClient;
use flare_grpc_proto::{
    GenericRequest, ProtoHookInvocationContext, ProtoHookMessageDraft, ProtoPreSendHookRequest,
    ProtoPreSendHookResponse,
};
use flare_server_core::error::{ErrorBuilder, ErrorCode, Result};
use flare_server_core::grpc::client::set_signed_context_token;
use flare_server_core::grpc::middleware::signed_context::VerifiedContext;
use prost::Message;
use tonic::transport::Channel;

use crate::discovery::{invalidate_static_channel_on_transport_failure, resolve_grpc_channel};

use super::super::types::{MessageDraft, PreSendDecision, PreSendHook, pre_send_rejection};

#[derive(Clone)]
pub struct GrpcHookFactory;

async fn hook_plugin_call<Mres: Message + Default>(
    client: &mut HookPluginClient<Channel>,
    ctx: &Ctx,
    operation: &'static str,
    request_type_url: &str,
    inner: &impl Message,
    static_metadata: &HashMap<String, String>,
) -> std::result::Result<Mres, tonic::Status> {
    let verified = ctx
        .get_data::<VerifiedContext>()
        .ok_or_else(|| tonic::Status::unauthenticated("verified hook identity required"))?;
    let claims = verified
        .claims()
        .ok_or_else(|| tonic::Status::unauthenticated("signed hook identity required"))?;
    let token = verified
        .token()
        .ok_or_else(|| tonic::Status::unauthenticated("signed hook token required"))?;
    if ctx.request_id() != claims.request_id || ctx.tenant_id() != Some(claims.tenant_id.as_str()) {
        return Err(tonic::Status::permission_denied(
            "hook identity context mismatch",
        ));
    }
    if claims.actor_type != flare_server_core::auth::SignedActorType::User
        || ctx.user_id() != Some(claims.actor_id.as_str())
    {
        return Err(tonic::Status::permission_denied(
            "pre-send hook requires signed user",
        ));
    }
    let any = prost_types::Any {
        type_url: request_type_url.to_string(),
        value: inner.encode_to_vec(),
    };
    let mut req = tonic::Request::new(GenericRequest {
        operation: operation.to_string(),
        metadata: static_metadata.clone(),
        payload: Some(any),
        request_id: claims.request_id.clone(),
    });
    set_signed_context_token(&mut req, token)?;
    let envelope = client.call(req).await?.into_inner();
    if !envelope.ok {
        return Err(tonic::Status::internal(format!(
            "{}: {}",
            envelope.error_code, envelope.error_message
        )));
    }
    let p = envelope
        .payload
        .ok_or_else(|| tonic::Status::internal("empty HookPlugin response payload"))?;
    Mres::decode(p.value.as_slice())
        .map_err(|e| tonic::Status::internal(format!("decode HookPlugin response: {e}")))
}

impl GrpcHookFactory {
    pub fn new() -> Self {
        Self
    }

    pub fn build_pre_send(
        &self,
        metadata: HashMap<String, String>,
        grpc_endpoint: String,
    ) -> Arc<dyn PreSendHook> {
        Arc::new(GrpcPreSendHook {
            grpc_endpoint,
            static_metadata: metadata,
        })
    }
}

/// 测试调用的结果：`Ok` 是目标的回答，`Err` 是调用失败。
pub(in crate::hooks) async fn test_pre_send(
    endpoint: &str,
    static_metadata: &HashMap<String, String>,
    ctx: &Ctx,
    draft: &MessageDraft,
) -> std::result::Result<ProtoPreSendHookResponse, tonic::Status> {
    let channel = hook_plugin_channel(endpoint)
        .await
        .map_err(|_| tonic::Status::unavailable("hook gRPC channel resolve failed"))?;
    let mut client = HookPluginClient::new(channel);
    let any = prost_types::Any {
        type_url: "type.googleapis.com/flare.capability.v1.PreSendHookRequest".to_string(),
        value: ProtoPreSendHookRequest {
            context: Some(build_context(ctx, static_metadata)),
            draft: Some(build_draft(draft)),
        }
        .encode_to_vec(),
    };
    // 不带签名上下文：发送路径转发的是发送者的身份，这里没有发送者；管理员的 Staff 上下文也不能发给
    // 一个还没保存的目标。校验身份的目标回 UNAUTHENTICATED，调用方算作「通了，要身份」。
    let envelope = client
        .call(tonic::Request::new(GenericRequest {
            operation: "flare.hook.v1.pre_send".to_string(),
            metadata: static_metadata.clone(),
            payload: Some(any),
            request_id: ctx.request_id().to_string(),
        }))
        .await
        .inspect_err(|status| forget_channel_on_transport_failure(endpoint, status))?
        .into_inner();
    if !envelope.ok {
        return Err(tonic::Status::internal("hook answered an error envelope"));
    }
    let payload = envelope
        .payload
        .ok_or_else(|| tonic::Status::internal("empty HookPlugin response payload"))?;
    ProtoPreSendHookResponse::decode(payload.value.as_slice())
        .map_err(|_| tonic::Status::internal("undecodable HookPlugin response"))
}

impl Default for GrpcHookFactory {
    fn default() -> Self {
        Self::new()
    }
}

/// 调用失败出在传输层（连不上、连接被断、HTTP/2 ping 超时）时丢弃缓存的静态通道，下一次
/// 调用重新解析地址、新建连接；对端回的业务状态不动缓存。
fn forget_channel_on_transport_failure(endpoint: &str, status: &tonic::Status) {
    if invalidate_static_channel_on_transport_failure(endpoint, status) {
        tracing::debug!(
            endpoint,
            code = ?status.code(),
            "hook channel dropped after transport failure"
        );
    }
}

async fn hook_plugin_channel(endpoint: &str) -> Result<Channel> {
    resolve_grpc_channel(endpoint).await.map_err(|e| {
        ErrorBuilder::new(
            ErrorCode::ServiceUnavailable,
            "hook gRPC channel resolve failed",
        )
        .details(e)
        .build_error()
    })
}

/// Keep the validated machine reason and give older clients a readable fallback.
/// Hook diagnostics stay private, and invalid codes retain the legacy response.
fn pre_send_hook_rejection(
    response: &ProtoPreSendHookResponse,
) -> flare_server_core::error::FlareError {
    let reason = response.deny_reason_code.as_str();
    let mut bytes = reason.bytes();
    let error = if reason.len() <= 64
        && bytes.next().is_some_and(|b| b.is_ascii_uppercase())
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    {
        ErrorBuilder::new(ErrorCode::OperationFailed, reason).details("服务端拒绝发送此消息")
    } else {
        ErrorBuilder::new(ErrorCode::OperationFailed, "pre-send hook rejected")
            .details("allow=false")
    };
    pre_send_rejection(error)
}

#[derive(Clone)]
struct GrpcPreSendHook {
    grpc_endpoint: String,
    static_metadata: HashMap<String, String>,
}

#[async_trait]
impl PreSendHook for GrpcPreSendHook {
    async fn handle(&self, ctx: &Ctx, draft: &mut MessageDraft) -> PreSendDecision {
        let channel = match hook_plugin_channel(&self.grpc_endpoint).await {
            Ok(ch) => ch,
            Err(err) => return PreSendDecision::Reject { error: err },
        };
        let mut client = HookPluginClient::new(channel);
        let request = ProtoPreSendHookRequest {
            context: Some(build_context(ctx, &self.static_metadata)),
            draft: Some(build_draft(draft)),
        };

        let response = hook_plugin_call::<ProtoPreSendHookResponse>(
            &mut client,
            ctx,
            "flare.hook.v1.pre_send",
            "type.googleapis.com/flare.capability.v1.PreSendHookRequest",
            &request,
            &self.static_metadata,
        )
        .await;
        match response {
            Ok(inner) => {
                if !inner.allow {
                    return PreSendDecision::Reject {
                        error: pre_send_hook_rejection(&inner),
                    };
                }
                if let Some(draft_resp) = inner.draft {
                    apply_draft(draft, draft_resp);
                }
                PreSendDecision::Continue
            }
            Err(status) => {
                forget_channel_on_transport_failure(&self.grpc_endpoint, &status);
                let err = ErrorBuilder::new(ErrorCode::ServiceUnavailable, "pre-send hook failed")
                    .details(status.to_string())
                    .build_error();
                PreSendDecision::Reject { error: err }
            }
        }
    }
}

fn build_context(
    ctx: &Ctx,
    static_metadata: &HashMap<String, String>,
) -> ProtoHookInvocationContext {
    use crate::hooks::hook_context_data::get_hook_context_data;

    let hook_data = get_hook_context_data(ctx).cloned().unwrap_or_default();
    let corridor = hook_data
        .attributes
        .get("corridor")
        .cloned()
        .or_else(|| hook_data.conversation_type.clone())
        .unwrap_or_else(|| "messaging".to_string());

    let mut attributes = hook_data.attributes.clone();
    for (key, value) in static_metadata {
        attributes
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    for (key, value) in &hook_data.request_metadata {
        attributes
            .entry(format!("request.{key}"))
            .or_insert_with(|| value.clone());
    }

    let operator_user_id = ctx
        .actor()
        .map(|a| a.actor_id().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| ctx.user_id().map(|s| s.to_string()))
        .unwrap_or_default();

    ProtoHookInvocationContext {
        conversation_id: hook_data.conversation_id.clone().unwrap_or_default(),
        conversation_type: hook_data.conversation_type.clone().unwrap_or_default(),
        corridor,
        tags: hook_data.tags.clone(),
        attributes,
        tenant_id: ctx.tenant_id().unwrap_or("").to_string(),
        app_id: String::new(),
        operator_user_id,
        request_id: ctx.request_id().to_string(),
        extension_bag: None,
    }
}

fn build_draft(draft: &MessageDraft) -> ProtoHookMessageDraft {
    ProtoHookMessageDraft {
        message_id: draft.message_id.clone().unwrap_or_default(),
        client_message_id: draft.client_message_id.clone().unwrap_or_default(),
        conversation_id: draft.conversation_id.clone().unwrap_or_default(),
        payload: draft.payload.clone(),
        headers: draft.headers.clone(),
        metadata: draft.metadata.clone(),
        message_type: String::new(),
        parent_message_id: String::new(),
        is_silent: false,
        extension_bag: None,
    }
}

fn apply_draft(target: &mut MessageDraft, source: ProtoHookMessageDraft) {
    if !source.message_id.is_empty() {
        target.message_id = Some(source.message_id);
    }
    if !source.client_message_id.is_empty() {
        target.client_message_id = Some(source.client_message_id);
    }
    if !source.conversation_id.is_empty() {
        target.conversation_id = Some(source.conversation_id);
    }
    target.payload = source.payload;
    target.headers = source.headers;
    target.metadata = source.metadata;
}

#[cfg(test)]
mod tests {
    /// `allow=false` 是策略拒绝：带进程内标记，错误码、原因与说明（客户端看到的）同以前；
    /// 调用失败（扩展不可用）不带标记。
    #[test]
    fn allow_false_is_marked_as_a_policy_rejection_with_the_same_wire_error() {
        use crate::hooks::is_pre_send_rejection;
        use flare_server_core::error::{ErrorBuilder, ErrorCode, FlareError};

        let rejection = super::pre_send_hook_rejection(&Default::default());
        assert!(is_pre_send_rejection(&rejection));
        match &rejection {
            FlareError::Localized {
                code,
                reason,
                details,
                ..
            } => {
                assert_eq!(*code, ErrorCode::OperationFailed);
                assert_eq!(reason, "pre-send hook rejected");
                assert_eq!(details.as_deref(), Some("allow=false"));
            }
            other => panic!("unexpected rejection shape: {other}"),
        }
        let detail = flare_server_core::error::to_error_detail(&rejection, "");
        assert_eq!(detail.code, ErrorCode::OperationFailed.as_u32() as i32);
        assert_eq!(detail.reason, "pre-send hook rejected");
        assert_eq!(detail.message, "allow=false");

        let unavailable = ErrorBuilder::new(ErrorCode::ServiceUnavailable, "pre-send hook failed")
            .details("transport error")
            .build_error();
        assert!(!is_pre_send_rejection(&unavailable));
    }

    #[test]
    fn hook_rejection_preserves_only_valid_machine_reasons() {
        use crate::hooks::is_pre_send_rejection;
        use flare_server_core::error::to_error_detail;

        let mut response = super::ProtoPreSendHookResponse {
            deny_reason_code: "GROUP_WRITE_DISABLED".into(),
            deny_reason_message: "operator-only diagnostic".into(),
            ..Default::default()
        };
        let rejection = super::pre_send_hook_rejection(&response);
        assert!(is_pre_send_rejection(&rejection));
        let detail = to_error_detail(&rejection, "");
        assert_eq!(detail.reason, "GROUP_WRITE_DISABLED");
        assert_eq!(detail.message, "服务端拒绝发送此消息");

        for invalid in [
            "bad_reason",
            "UPPER CASE",
            "1DENIED",
            "DENIED\n",
            &"A".repeat(65),
        ] {
            response.deny_reason_code = invalid.into();
            let detail = to_error_detail(&super::pre_send_hook_rejection(&response), "");
            assert_eq!(detail.reason, "pre-send hook rejected");
            assert_eq!(detail.message, "allow=false");
        }
    }
}
