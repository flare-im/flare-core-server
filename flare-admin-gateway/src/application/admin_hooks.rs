//! Hook 治理路由（`/hooks/**`）的 HTTP 契约，以及与 `CapabilityService.Administer` 载荷
//! （`flare.capability.v1.*HookConfig*` 消息）的双向映射。
//!
//! 一条 Hook = 一个点位（`hook_type`）+ 一个目标，加名称、选择器、`order`、`required` 和超时。
//! 范围永远来自已验签的 Staff 上下文：`/hooks/**` 是签名里的应用，`/platform/hooks/**` 是平台
//! （`tenant_id` 为空），调用方给的一律忽略；按 hook_id 的操作由 Capability 先核对该配置的范围。时间
//! 统一为 Unix 毫秒。webhook 的签名密钥只进不出：读回的传输配置不含 `secret`，只说明是否已配置。

use std::collections::HashMap;

use flare_grpc_proto::capability::{
    CreateHookConfigRequest, DeleteHookConfigResponse, DeploymentHook, GetHookStatisticsRequest,
    HookConfig, HookExecution, HookPointEntry, HookProfile, HookSelector, HookStatistics,
    HookTransport, ListHookConfigsRequest, ListHookConfigsResponse, ListHookPointsResponse,
    ListHookProfilesResponse, QueryHookExecutionsRequest, QueryHookExecutionsResponse,
    SetHookStatusRequest, SetHookStatusResponse, TestHookConfigRequest, TestHookConfigResponse,
    UpdateHookConfigRequest,
};
use flare_proto::common::{Pagination, TimeRange};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 列表与执行记录的页大小：缺省 100，上限 1000（Capability 同样截到 1000）。
pub const DEFAULT_HOOK_PAGE: i32 = 100;
pub const MAX_HOOK_PAGE: i32 = 1000;

/// 平台范围的 `tenant_id`。
pub const PLATFORM_TENANT: &str = "";

/// `Administer` 的 operation 名。
pub mod operation {
    pub const CREATE: &str = "flare.extension.v1.hook_config.create";
    pub const GET: &str = "flare.extension.v1.hook_config.get";
    pub const UPDATE: &str = "flare.extension.v1.hook_config.update";
    pub const LIST: &str = "flare.extension.v1.hook_config.list";
    pub const DELETE: &str = "flare.extension.v1.hook_config.delete";
    pub const SET_STATUS: &str = "flare.extension.v1.hook_config.set_status";
    pub const GET_STATISTICS: &str = "flare.extension.v1.hook_config.get_statistics";
    pub const QUERY_EXECUTIONS: &str = "flare.extension.v1.hook_config.query_executions";
    pub const TEST: &str = "flare.extension.v1.hook_config.test";
    pub const POINT_LIST: &str = "flare.extension.v1.hook_point.list";
    pub const PROFILE_LIST: &str = "flare.extension.v1.hook_profile.list";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminHookArgError {
    pub code: &'static str,
    pub message: String,
}

fn invalid(code: &'static str, message: impl Into<String>) -> AdminHookArgError {
    AdminHookArgError {
        code,
        message: message.into(),
    }
}

/// 传输配置。读回时 `secret` 不出网关：`secret_configured` 说明 Capability 持有的配置里有无签名密钥。
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct AdminHookTransportHttp {
    /// `grpc` / `webhook`
    #[serde(rename = "type", default)]
    pub transport_type: String,
    #[serde(default)]
    pub service_name: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub registry_type: String,
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub load_balance: String,
    /// 只在创建与更新时接受；读回时省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// 读回时：Capability 持有的配置是否带签名密钥。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_configured: Option<bool>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// 这条 Hook 的超时，毫秒，1 到 30000。
    #[serde(default)]
    pub timeout_ms: i32,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// 运行时实际判断的三个字段，空表示全部。
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct AdminHookSelectorHttp {
    #[serde(default)]
    pub tenants: Vec<String>,
    #[serde(default)]
    pub conversation_types: Vec<String>,
    #[serde(default)]
    pub message_types: Vec<String>,
}

/// 一条 Hook 配置（读回形状）。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookHttp {
    pub hook_id: String,
    pub name: String,
    pub hook_type: String,
    /// 应用 ID；空为平台 Hook。
    pub tenant_id: String,
    /// 同一来源内的先后，0 到 1000，小的先执行。
    pub order: i32,
    /// 必需：目标不可用时回可重试的 503；否则跳过。
    pub required: bool,
    pub enabled: bool,
    pub transport: Option<AdminHookTransportHttp>,
    pub selector: Option<AdminHookSelectorHttp>,
    /// Unix 毫秒。
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookDetailHttp {
    pub hook: AdminHookHttp,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHooksPageHttp {
    pub hooks: Vec<AdminHookHttp>,
    /// 本页实际使用的页大小。
    pub limit: i32,
    pub has_more: bool,
    /// 过滤后的总数。
    pub total: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
pub struct AdminHookListQuery {
    #[serde(default)]
    pub hook_type: Option<String>,
    #[serde(default)]
    pub enabled_only: Option<bool>,
    #[serde(default)]
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AdminHookCreateHttpRequest {
    pub name: String,
    pub hook_type: String,
    #[serde(default)]
    pub order: i32,
    /// 缺省为必需。
    #[serde(default = "required_by_default")]
    pub required: bool,
    #[serde(default)]
    pub transport: Option<AdminHookTransportHttp>,
    #[serde(default)]
    pub selector: Option<AdminHookSelectorHttp>,
}

fn required_by_default() -> bool {
    true
}

/// 没给的字段不变。
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AdminHookUpdateHttpRequest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub order: Option<i32>,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub transport: Option<AdminHookTransportHttp>,
    #[serde(default)]
    pub selector: Option<AdminHookSelectorHttp>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AdminHookStatusHttpRequest {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookStatusHttp {
    pub hook_id: String,
    pub enabled: bool,
    pub updated: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookDeleteHttp {
    pub hook_id: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
pub struct AdminHookStatisticsQuery {
    /// Unix 毫秒；两者都缺省时 Capability 取最近 24 小时。
    #[serde(default)]
    pub start_time: Option<i64>,
    #[serde(default)]
    pub end_time: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookStatisticsHttp {
    pub hook_id: String,
    pub total_executions: i64,
    pub success_count: i64,
    pub failure_count: i64,
    pub avg_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub rate_limit_count: i64,
    pub circuit_break_count: i64,
    pub error_count_by_code: HashMap<String, i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
pub struct AdminHookExecutionsQuery {
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub success_only: Option<bool>,
    /// Unix 毫秒。
    #[serde(default)]
    pub start_time: Option<i64>,
    #[serde(default)]
    pub end_time: Option<i64>,
    #[serde(default)]
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookExecutionHttp {
    pub execution_id: String,
    pub hook_id: String,
    pub message_id: String,
    pub success: bool,
    pub latency_ms: i32,
    pub error_code: String,
    pub error_message: String,
    /// Unix 毫秒。
    pub executed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookExecutionsPageHttp {
    pub hook_id: String,
    pub executions: Vec<AdminHookExecutionHttp>,
    pub limit: i32,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookPointHttp {
    /// 对外的点位名，如 `message.pre_send`。
    pub id: String,
    /// 存储名，创建时的 `hook_type`。
    pub hook_type: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookPointsHttp {
    pub points: Vec<AdminHookPointHttp>,
}

/// 部署文件里的一条 Hook，只读。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminDeploymentHookHttp {
    pub name: String,
    pub hook_type: String,
    pub description: String,
    pub enabled: bool,
    pub order: i32,
    pub required: bool,
    pub timeout_ms: i64,
    /// `grpc` / `webhook` / `local`
    pub target_type: String,
    pub target_endpoint: String,
    pub selector: Option<AdminHookSelectorHttp>,
}

/// 一个服务装载的部署 Hook。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookProfileHttp {
    pub service: String,
    pub source: String,
    pub instance_id: String,
    /// Unix 毫秒。
    pub loaded_at: Option<i64>,
    pub hooks: Vec<AdminDeploymentHookHttp>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookProfilesHttp {
    pub profiles: Vec<AdminHookProfileHttp>,
}

/// 测试调用：给 `hook_id` 测已存的目标，`transport` 测一个还没保存的目标；都给时目标变了就不沿用
/// 已存的密钥和请求头。
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct AdminHookTestHttpRequest {
    #[serde(default)]
    pub hook_id: String,
    #[serde(default)]
    pub hook_type: String,
    #[serde(default)]
    pub transport: Option<AdminHookTransportHttp>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminHookTestHttp {
    /// `allowed` / `rewritten` / `rejected` / `identity_required` / `failed`
    pub outcome: String,
    pub latency_ms: i64,
    /// 目标给的拒绝原因码，或失败类别（`timeout`、`unavailable`、`bad_response`）。
    pub code: String,
}

fn timestamp_ms(ts: &prost_types::Timestamp) -> i64 {
    ts.seconds
        .saturating_mul(1000)
        .saturating_add(i64::from(ts.nanos) / 1_000_000)
}

/// 页大小：缺省 100，小于 1 拒绝，大于 1000 截到 1000。
pub fn hook_page_limit(requested: Option<i32>) -> Result<i32, AdminHookArgError> {
    match requested {
        None => Ok(DEFAULT_HOOK_PAGE),
        Some(limit) if limit < 1 => Err(invalid(
            "ADMIN_PAGE_LIMIT_INVALID",
            format!("limit must be between 1 and {MAX_HOOK_PAGE}"),
        )),
        Some(limit) => Ok(limit.min(MAX_HOOK_PAGE)),
    }
}

/// Capability 读回时在 `transport.metadata` 里说明签名密钥是否已设置的键。
const SECRET_CONFIGURED_KEY: &str = "signing_secret_configured";

fn transport_from_http(transport: AdminHookTransportHttp) -> HookTransport {
    HookTransport {
        r#type: transport.transport_type,
        service_name: transport.service_name,
        endpoint: transport.endpoint,
        registry_type: transport.registry_type,
        namespace: transport.namespace,
        load_balance: transport.load_balance,
        secret: transport.secret.unwrap_or_default(),
        headers: transport.headers,
        timeout_ms: transport.timeout_ms,
        metadata: transport.metadata,
    }
}

fn transport_to_http(transport: HookTransport) -> AdminHookTransportHttp {
    let secret_configured = transport
        .metadata
        .get(SECRET_CONFIGURED_KEY)
        .is_some_and(|value| value == "true");
    AdminHookTransportHttp {
        transport_type: transport.r#type,
        service_name: transport.service_name,
        endpoint: transport.endpoint,
        registry_type: transport.registry_type,
        namespace: transport.namespace,
        load_balance: transport.load_balance,
        secret: None,
        secret_configured: Some(secret_configured),
        headers: HashMap::new(),
        timeout_ms: transport.timeout_ms,
        metadata: HashMap::new(),
    }
}

fn selector_from_http(selector: AdminHookSelectorHttp) -> HookSelector {
    HookSelector {
        tenants: selector.tenants,
        conversation_types: selector.conversation_types,
        message_types: selector.message_types,
    }
}

fn selector_to_http(selector: HookSelector) -> AdminHookSelectorHttp {
    AdminHookSelectorHttp {
        tenants: selector.tenants,
        conversation_types: selector.conversation_types,
        message_types: selector.message_types,
    }
}

pub fn hook_http(config: HookConfig) -> AdminHookHttp {
    AdminHookHttp {
        hook_id: config.hook_id,
        name: config.name,
        hook_type: config.hook_type,
        tenant_id: config.tenant_id,
        order: config.order,
        required: config.required,
        enabled: config.enabled,
        transport: config.transport.map(transport_to_http),
        selector: config.selector.map(selector_to_http),
        created_at: config.created_at.as_ref().map(timestamp_ms),
        updated_at: config.updated_at.as_ref().map(timestamp_ms),
    }
}

pub fn build_create_request(
    tenant_id: &str,
    request: AdminHookCreateHttpRequest,
) -> Result<CreateHookConfigRequest, AdminHookArgError> {
    if request.name.trim().is_empty() {
        return Err(invalid("ADMIN_HOOK_NAME_REQUIRED", "name is required"));
    }
    if request.hook_type.trim().is_empty() {
        return Err(invalid("ADMIN_HOOK_TYPE_REQUIRED", "hook_type is required"));
    }
    Ok(CreateHookConfigRequest {
        name: request.name,
        hook_type: request.hook_type,
        tenant_id: tenant_id.to_string(),
        order: request.order,
        required: request.required,
        transport: request.transport.map(transport_from_http),
        selector: request.selector.map(selector_from_http),
    })
}

pub fn build_update_request(
    hook_id: &str,
    request: AdminHookUpdateHttpRequest,
) -> UpdateHookConfigRequest {
    UpdateHookConfigRequest {
        hook_id: hook_id.to_string(),
        name: request.name,
        order: request.order,
        required: request.required,
        transport: request.transport.map(transport_from_http),
        selector: request.selector.map(selector_from_http),
    }
}

pub fn build_test_request(
    request: AdminHookTestHttpRequest,
) -> Result<TestHookConfigRequest, AdminHookArgError> {
    if request.hook_id.is_empty() && request.transport.is_none() {
        return Err(invalid(
            "ADMIN_HOOK_TARGET_REQUIRED",
            "hook_id or transport is required",
        ));
    }
    Ok(TestHookConfigRequest {
        hook_id: request.hook_id,
        hook_type: request.hook_type,
        transport: request.transport.map(transport_from_http),
    })
}

pub fn test_http(response: TestHookConfigResponse) -> AdminHookTestHttp {
    AdminHookTestHttp {
        outcome: response.outcome,
        latency_ms: response.latency_ms,
        code: response.code,
    }
}

fn point_http(point: HookPointEntry) -> AdminHookPointHttp {
    AdminHookPointHttp {
        id: point.id,
        hook_type: point.hook_type,
        summary: point.summary,
    }
}

pub fn points_http(response: ListHookPointsResponse) -> AdminHookPointsHttp {
    AdminHookPointsHttp {
        points: response.points.into_iter().map(point_http).collect(),
    }
}

fn deployment_hook_http(hook: DeploymentHook) -> AdminDeploymentHookHttp {
    AdminDeploymentHookHttp {
        name: hook.name,
        hook_type: hook.hook_type,
        description: hook.description,
        enabled: hook.enabled,
        order: hook.order,
        required: hook.required,
        timeout_ms: hook.timeout_ms,
        target_type: hook.target_type,
        target_endpoint: hook.target_endpoint,
        selector: hook.selector.map(selector_to_http),
    }
}

fn profile_http(profile: HookProfile) -> AdminHookProfileHttp {
    AdminHookProfileHttp {
        service: profile.service,
        source: profile.source,
        instance_id: profile.instance_id,
        loaded_at: profile.loaded_at.as_ref().map(timestamp_ms),
        hooks: profile
            .hooks
            .into_iter()
            .map(deployment_hook_http)
            .collect(),
    }
}

pub fn profiles_http(response: ListHookProfilesResponse) -> AdminHookProfilesHttp {
    AdminHookProfilesHttp {
        profiles: response.profiles.into_iter().map(profile_http).collect(),
    }
}

pub fn build_list_request(
    tenant_id: &str,
    query: &AdminHookListQuery,
) -> Result<(ListHookConfigsRequest, i32), AdminHookArgError> {
    let limit = hook_page_limit(query.limit)?;
    Ok((
        ListHookConfigsRequest {
            tenant_id: tenant_id.to_string(),
            hook_type: query.hook_type.clone().unwrap_or_default(),
            enabled_only: query.enabled_only.unwrap_or(false),
            pagination: Some(Pagination {
                limit,
                ..Default::default()
            }),
        },
        limit,
    ))
}

pub fn hooks_page_http(response: ListHookConfigsResponse, limit: i32) -> AdminHooksPageHttp {
    let (has_more, total) = response
        .pagination
        .as_ref()
        .map(|pagination| (pagination.has_more, pagination.total_size))
        .unwrap_or((false, response.configs.len() as i64));
    AdminHooksPageHttp {
        hooks: response.configs.into_iter().map(hook_http).collect(),
        limit,
        has_more,
        total,
    }
}

pub fn build_status_request(
    hook_id: &str,
    request: AdminHookStatusHttpRequest,
) -> SetHookStatusRequest {
    SetHookStatusRequest {
        hook_id: hook_id.to_string(),
        enabled: request.enabled,
    }
}

pub fn status_http(
    hook_id: String,
    enabled: bool,
    response: SetHookStatusResponse,
) -> AdminHookStatusHttp {
    AdminHookStatusHttp {
        hook_id,
        enabled,
        updated: response.success,
    }
}

pub fn delete_http(hook_id: String, response: DeleteHookConfigResponse) -> AdminHookDeleteHttp {
    AdminHookDeleteHttp {
        hook_id,
        deleted: response.success,
    }
}

fn time_range(
    start: Option<i64>,
    end: Option<i64>,
) -> Result<Option<TimeRange>, AdminHookArgError> {
    if let (Some(start), Some(end)) = (start, end)
        && end < start
    {
        return Err(invalid(
            "ADMIN_HOOK_TIME_RANGE_INVALID",
            "end_time must not be earlier than start_time",
        ));
    }
    Ok((start.is_some() || end.is_some()).then_some(TimeRange {
        start_time: start,
        end_time: end,
    }))
}

pub fn build_statistics_request(
    hook_id: &str,
    query: &AdminHookStatisticsQuery,
) -> Result<GetHookStatisticsRequest, AdminHookArgError> {
    Ok(GetHookStatisticsRequest {
        hook_id: hook_id.to_string(),
        time_range: time_range(query.start_time, query.end_time)?,
    })
}

pub fn statistics_http(
    hook_id: String,
    statistics: Option<HookStatistics>,
) -> AdminHookStatisticsHttp {
    let statistics = statistics.unwrap_or_default();
    AdminHookStatisticsHttp {
        hook_id,
        total_executions: statistics.total_executions,
        success_count: statistics.success_count,
        failure_count: statistics.failure_count,
        avg_latency_ms: statistics.avg_latency_ms,
        p99_latency_ms: statistics.p99_latency_ms,
        rate_limit_count: statistics.rate_limit_count,
        circuit_break_count: statistics.circuit_break_count,
        error_count_by_code: statistics.error_count_by_code,
    }
}

pub fn build_executions_request(
    hook_id: &str,
    query: &AdminHookExecutionsQuery,
) -> Result<(QueryHookExecutionsRequest, i32), AdminHookArgError> {
    let limit = hook_page_limit(query.limit)?;
    Ok((
        QueryHookExecutionsRequest {
            hook_id: hook_id.to_string(),
            message_id: query.message_id.clone().unwrap_or_default(),
            success_only: query.success_only.unwrap_or(false),
            time_range: time_range(query.start_time, query.end_time)?,
            pagination: Some(Pagination {
                limit,
                ..Default::default()
            }),
        },
        limit,
    ))
}

fn execution_http(execution: HookExecution) -> AdminHookExecutionHttp {
    AdminHookExecutionHttp {
        execution_id: execution.execution_id,
        hook_id: execution.hook_id,
        message_id: execution.message_id,
        success: execution.success,
        latency_ms: execution.latency_ms,
        error_code: execution.error_code,
        error_message: execution.error_message,
        executed_at: execution.executed_at.as_ref().map(timestamp_ms),
    }
}

pub fn executions_page_http(
    hook_id: String,
    response: QueryHookExecutionsResponse,
    limit: i32,
) -> AdminHookExecutionsPageHttp {
    let has_more = response
        .pagination
        .as_ref()
        .is_some_and(|pagination| pagination.has_more);
    AdminHookExecutionsPageHttp {
        hook_id,
        executions: response
            .executions
            .into_iter()
            .map(execution_http)
            .collect(),
        limit,
        has_more,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_body() -> AdminHookCreateHttpRequest {
        AdminHookCreateHttpRequest {
            name: "dlp".into(),
            hook_type: "pre_send".into(),
            order: 10,
            required: false,
            transport: Some(AdminHookTransportHttp {
                transport_type: "webhook".into(),
                endpoint: "https://hooks.example/dlp".into(),
                secret: Some("s3cret".into()),
                ..Default::default()
            }),
            selector: Some(AdminHookSelectorHttp {
                message_types: vec!["text".into()],
                ..Default::default()
            }),
        }
    }

    #[test]
    fn create_requests_take_the_scope_from_the_caller_and_validate_names() {
        let proto = build_create_request(PLATFORM_TENANT, create_body()).unwrap();
        assert_eq!(proto.tenant_id, "");
        assert_eq!(proto.order, 10);
        assert!(!proto.required);
        assert_eq!(proto.transport.as_ref().unwrap().secret, "s3cret");
        assert_eq!(proto.selector.unwrap().message_types, vec!["text"]);

        let unnamed = AdminHookCreateHttpRequest {
            name: " ".into(),
            ..create_body()
        };
        assert_eq!(
            build_create_request(PLATFORM_TENANT, unnamed)
                .unwrap_err()
                .code,
            "ADMIN_HOOK_NAME_REQUIRED"
        );
    }

    /// 缺省必需；已删的字段不再是请求的一部分，写了也不进 proto。
    #[test]
    fn a_hook_is_required_unless_it_says_otherwise() {
        let body: AdminHookCreateHttpRequest =
            serde_json::from_str(r#"{"name":"a","hook_type":"pre_send"}"#).unwrap();
        assert!(body.required);
        let update: AdminHookUpdateHttpRequest = serde_json::from_str(r#"{"order":0}"#).unwrap();
        let update = build_update_request("7", update);
        assert_eq!(update.order, Some(0));
        assert_eq!(update.required, None);
    }

    #[test]
    fn a_test_call_needs_a_target() {
        assert_eq!(
            build_test_request(AdminHookTestHttpRequest::default())
                .unwrap_err()
                .code,
            "ADMIN_HOOK_TARGET_REQUIRED"
        );
        let saved = AdminHookTestHttpRequest {
            hook_id: "7".into(),
            ..Default::default()
        };
        assert_eq!(build_test_request(saved).unwrap().hook_id, "7");
    }

    #[test]
    fn read_back_hooks_hide_the_secret_and_use_millis() {
        let incoming = transport_from_http(AdminHookTransportHttp {
            transport_type: "grpc".into(),
            headers: HashMap::from([("x-api-key".into(), "write-canary".into())]),
            metadata: HashMap::from([("authorization".into(), "metadata-write-canary".into())]),
            ..Default::default()
        });
        assert_eq!(incoming.headers["x-api-key"], "write-canary");
        assert_eq!(incoming.metadata["authorization"], "metadata-write-canary");
        let hook = hook_http(HookConfig {
            hook_id: "7".into(),
            name: "dlp".into(),
            hook_type: "pre_send".into(),
            tenant_id: "".into(),
            order: 10,
            required: true,
            enabled: true,
            transport: Some(HookTransport {
                r#type: "webhook".into(),
                secret: "s3cret".into(),
                headers: HashMap::from([("authorization".into(), "header-canary".into())]),
                metadata: HashMap::from([
                    ("x-api-key".into(), "metadata-canary".into()),
                    (SECRET_CONFIGURED_KEY.into(), "true".into()),
                ]),
                ..Default::default()
            }),
            selector: None,
            created_at: Some(prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: 250_000_000,
            }),
            updated_at: None,
        });
        assert_eq!(hook.order, 10);
        assert!(hook.required);
        let transport = hook.transport.unwrap();
        assert_eq!(transport.secret, None);
        assert_eq!(transport.secret_configured, Some(true));
        assert!(transport.headers.is_empty());
        assert!(transport.metadata.is_empty());
        assert_eq!(hook.created_at, Some(1_700_000_000_250));
        let json = serde_json::to_string(&transport).unwrap();
        assert!(!json.contains("s3cret"));
        assert!(!json.contains("canary"));
        assert!(!json.contains("\"secret\""));
    }

    #[test]
    fn paging_and_time_ranges_are_bounded() {
        assert_eq!(hook_page_limit(None), Ok(100));
        assert_eq!(hook_page_limit(Some(5000)), Ok(1000));
        assert_eq!(
            hook_page_limit(Some(0)).unwrap_err().code,
            "ADMIN_PAGE_LIMIT_INVALID"
        );
        let (list, limit) = build_list_request(
            "tenant-a",
            &AdminHookListQuery {
                hook_type: Some("pre_send".into()),
                enabled_only: Some(true),
                limit: Some(20),
            },
        )
        .unwrap();
        assert_eq!(limit, 20);
        assert_eq!(list.tenant_id, "tenant-a");
        assert!(list.enabled_only);
        assert_eq!(list.pagination.unwrap().limit, 20);
        assert_eq!(
            build_statistics_request(
                "7",
                &AdminHookStatisticsQuery {
                    start_time: Some(20),
                    end_time: Some(10),
                }
            )
            .unwrap_err()
            .code,
            "ADMIN_HOOK_TIME_RANGE_INVALID"
        );
        let statistics =
            build_statistics_request("7", &AdminHookStatisticsQuery::default()).unwrap();
        assert!(statistics.time_range.is_none());
        let (executions, _) = build_executions_request(
            "7",
            &AdminHookExecutionsQuery {
                start_time: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            executions.time_range.unwrap(),
            TimeRange {
                start_time: Some(10),
                end_time: None
            }
        );
    }
}
