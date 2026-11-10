//! 真库：Hook 配置经 `CapabilityService/Administer` 写入。签名密钥只写不读、更新时留空即沿用；每次写入
//! 在同一事务里递增范围版本，Ingest 据此热加载；平台 Hook 只能用平台写码写；应用 Hook 建不出来（H3），
//! 用例里的应用行直接写库；应用不能占用平台和部署 Hook 的名字。
//!
//! 需要 `FLARE_TENANT_TEST_DATABASE_URL`（用 `deploy/init.sql` 初始化的库）。用例标了 `#[ignore]`：
//! 默认的 `cargo test` 只记为 ignored，`-- --ignored` 显式运行时变量必须已设置，否则直接失败。只写随机
//! 租户与随机名的行，结束时（含断言失败的 unwind）删除。

use std::sync::Arc;

use flare_grpc_proto::capability::{
    CreateHookConfigRequest, CreateHookConfigResponse, DeleteHookConfigRequest, GenericRequest,
    GenericResponse, GetHookConfigRequest, GetHookConfigResponse, HookConfig as ProtoHookConfig,
    HookTransport, ListHookConfigsRequest, ListHookConfigsResponse, SetHookStatusRequest,
    UpdateHookConfigRequest, UpdateHookConfigResponse,
};
use prost::Message;

use super::administer::{HookAdministrator, dispatch_hook_administer, test_platform_employee};
use crate::domain::model::{HookConfigItem, HookSelectorConfig, HookTransportConfig};
use crate::infrastructure::persistence::PostgresHookConfigRepository;
use crate::interface::grpc::HookServiceServer;

const ENV_DATABASE_URL: &str = "FLARE_TENANT_TEST_DATABASE_URL";

fn database_url() -> String {
    std::env::var(ENV_DATABASE_URL).unwrap_or_else(|_| {
        panic!("set {ENV_DATABASE_URL} to a database initialised with deploy/init.sql")
    })
}

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// 用例结束时（正常结束或断言失败的 unwind）删掉本用例租户的行。`Drop` 里不能 await、测试的运行时里
/// 也不能 `block_on`，所以另起线程，用自己的运行时与单连接删完再返回。
struct TenantRowCleanup {
    url: String,
    tenant: String,
}

impl Drop for TenantRowCleanup {
    fn drop(&mut self) {
        let url = self.url.clone();
        let tenant = self.tenant.clone();
        let outcome = std::thread::spawn(move || -> Result<(), String> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            runtime.block_on(async move {
                let pool = sqlx::postgres::PgPoolOptions::new()
                    .max_connections(1)
                    .connect(&url)
                    .await
                    .map_err(|e| e.to_string())?;
                for sql in [
                    "DELETE FROM hook_configs WHERE tenant_id = $1",
                    "DELETE FROM hook_config_versions WHERE scope = $1",
                    "DELETE FROM hook_deployment_summary WHERE service = $1",
                ] {
                    sqlx::query(sql)
                        .bind(&tenant)
                        .execute(&pool)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                pool.close().await;
                Ok(())
            })
        })
        .join();
        if !matches!(outcome, Ok(Ok(()))) {
            eprintln!("hook admin test rows were not cleaned up: {outcome:?}");
        }
    }
}

fn cleanup(tenant: &str) -> TenantRowCleanup {
    TenantRowCleanup {
        url: database_url(),
        tenant: tenant.to_string(),
    }
}

fn employee(tenant: &str) -> HookAdministrator {
    super::administer::test_employee("employee-hooks", tenant, "request-hooks")
}

fn platform_employee() -> HookAdministrator {
    test_platform_employee("employee-hooks", "request-hooks")
}

/// 应用 Hook：Administer 建不出来，直接写行。
async fn seed(
    repository: &PostgresHookConfigRepository,
    tenant: &str,
    name: &str,
    endpoint: &str,
    secret: Option<&str>,
) -> String {
    let item = HookConfigItem {
        name: name.into(),
        description: None,
        enabled: true,
        order: 900,
        timeout_ms: 1000,
        require_success: true,
        selector: HookSelectorConfig::default(),
        transport: HookTransportConfig::Webhook {
            endpoint: endpoint.into(),
            secret: secret.map(str::to_string),
            headers: Default::default(),
        },
        metadata: Default::default(),
    };
    repository
        .save(Some(tenant), "pre_send", &item, None)
        .await
        .unwrap()
        .to_string()
}

fn administer(operation: &str, type_name: &str, value: Vec<u8>) -> GenericRequest {
    GenericRequest {
        operation: format!("flare.extension.v1.hook_config.{operation}"),
        request_id: String::new(),
        metadata: Default::default(),
        payload: Some(prost_types::Any {
            type_url: format!("type.googleapis.com/flare.capability.v1.{type_name}"),
            value,
        }),
    }
}

fn webhook_transport(endpoint: &str, secret: &str) -> HookTransport {
    HookTransport {
        r#type: "webhook".into(),
        endpoint: endpoint.into(),
        secret: secret.into(),
        timeout_ms: 1000,
        ..Default::default()
    }
}

fn create(tenant: &str, name: &str, transport: HookTransport) -> GenericRequest {
    administer(
        "create",
        "CreateHookConfigRequest",
        CreateHookConfigRequest {
            name: name.into(),
            hook_type: "pre_send".into(),
            tenant_id: tenant.into(),
            order: 900,
            transport: Some(transport),
            ..Default::default()
        }
        .encode_to_vec(),
    )
}

fn update_transport(hook_id: &str, transport: HookTransport) -> GenericRequest {
    administer(
        "update",
        "UpdateHookConfigRequest",
        UpdateHookConfigRequest {
            hook_id: hook_id.into(),
            transport: Some(transport),
            ..Default::default()
        }
        .encode_to_vec(),
    )
}

fn get(hook_id: &str) -> GenericRequest {
    administer(
        "get",
        "GetHookConfigRequest",
        GetHookConfigRequest {
            hook_id: hook_id.into(),
        }
        .encode_to_vec(),
    )
}

fn list(tenant: &str) -> GenericRequest {
    administer(
        "list",
        "ListHookConfigsRequest",
        ListHookConfigsRequest {
            tenant_id: tenant.into(),
            ..Default::default()
        }
        .encode_to_vec(),
    )
}

fn set_status(hook_id: &str, enabled: bool) -> GenericRequest {
    administer(
        "set_status",
        "SetHookStatusRequest",
        SetHookStatusRequest {
            hook_id: hook_id.into(),
            enabled,
        }
        .encode_to_vec(),
    )
}

fn delete(hook_id: &str) -> GenericRequest {
    administer(
        "delete",
        "DeleteHookConfigRequest",
        DeleteHookConfigRequest {
            hook_id: hook_id.into(),
        }
        .encode_to_vec(),
    )
}

async fn call(
    governance: &HookServiceServer,
    request: GenericRequest,
    tenant: &str,
) -> GenericResponse {
    let response = dispatch_hook_administer(governance, request, &employee(tenant))
        .await
        .unwrap()
        .into_inner();
    assert!(response.ok, "{response:?}");
    response
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// 应答里（含编码后的字节）没有密钥，只说明已设置。
fn assert_write_only(response: &GenericResponse, config: &ProtoHookConfig, secrets: &[&str]) {
    let bytes = response.encode_to_vec();
    for secret in secrets {
        assert!(!contains(&bytes, secret), "a signing secret was read back");
    }
    let transport = config.transport.as_ref().unwrap();
    assert!(transport.secret.is_empty());
    assert_eq!(
        transport
            .metadata
            .get("signing_secret_configured")
            .map(String::as_str),
        Some("true")
    );
}

async fn repository() -> Arc<PostgresHookConfigRepository> {
    Arc::new(
        PostgresHookConfigRepository::new(&database_url())
            .await
            .unwrap(),
    )
}

/// create、get、list 都不带密钥；照读回的配置写回（密钥为空）沿用原密钥；传新值即换钥；密钥照常存在
/// 库里，Ingest 要用它签名。
#[tokio::test]
#[ignore = "requires FLARE_TENANT_TEST_DATABASE_URL pointing at a database initialised with deploy/init.sql"]
async fn webhook_signing_secrets_are_write_only() {
    let repository = repository().await;
    let run = suffix();
    let tenant = format!("tw{run}");
    let _cleanup = cleanup(&tenant);
    let first_secret = format!("first-{run}");
    let rotated_secret = format!("rotated-{run}");
    let secrets = [first_secret.as_str(), rotated_secret.as_str()];
    let endpoint = "https://hooks.example.com/w";
    let governance = HookServiceServer::new(Arc::clone(&repository));

    let hook_id = seed(
        &repository,
        &tenant,
        &format!("hook-{run}"),
        endpoint,
        Some(&first_secret),
    )
    .await;

    let got = call(&governance, get(&hook_id), &tenant).await;
    let config = GetHookConfigResponse::decode(got.payload.as_ref().unwrap().value.as_slice())
        .unwrap()
        .config
        .unwrap();
    assert_write_only(&got, &config, &secrets);
    let listed = call(&governance, list(&tenant), &tenant).await;
    let configs =
        ListHookConfigsResponse::decode(listed.payload.as_ref().unwrap().value.as_slice())
            .unwrap()
            .configs;
    let ours: Vec<_> = configs.iter().filter(|c| c.hook_id == hook_id).collect();
    assert_eq!(ours.len(), 1);
    assert_write_only(&listed, ours[0], &secrets);

    // 照读回的配置写回：沿用原密钥。
    let read_back = config.transport.clone().unwrap();
    assert!(read_back.secret.is_empty());
    let updated = call(&governance, update_transport(&hook_id, read_back), &tenant).await;
    let config =
        UpdateHookConfigResponse::decode(updated.payload.as_ref().unwrap().value.as_slice())
            .unwrap()
            .config
            .unwrap();
    assert_write_only(&updated, &config, &secrets);
    let (_, stored) = repository
        .get_by_id(hook_id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        stored.transport,
        HookTransportConfig::Webhook { secret: Some(ref s), .. } if *s == first_secret
    ));

    // 换钥：传新的非空值。
    call(
        &governance,
        update_transport(&hook_id, webhook_transport(endpoint, &rotated_secret)),
        &tenant,
    )
    .await;
    let (_, stored) = repository
        .get_by_id(hook_id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        stored.transport,
        HookTransportConfig::Webhook { secret: Some(ref s), .. } if *s == rotated_secret
    ));
}

async fn scope_version(repository: &PostgresHookConfigRepository, tenant: &str) -> i64 {
    sqlx::query_scalar("SELECT version FROM hook_config_versions WHERE scope = $1")
        .bind(tenant)
        .fetch_one(repository.connection_pool().as_ref())
        .await
        .unwrap()
}

/// 每种写入都让这个应用的版本前进：Ingest 只取版本晚于已加载版本的范围，漏掉的写入永远不生效。
#[tokio::test]
#[ignore = "requires FLARE_TENANT_TEST_DATABASE_URL pointing at a database initialised with deploy/init.sql"]
async fn every_write_bumps_the_scope_version() {
    let repository = repository().await;
    let run = suffix();
    let tenant = format!("tv{run}");
    let _cleanup = cleanup(&tenant);
    let governance = HookServiceServer::new(Arc::clone(&repository));
    let endpoint = "https://hooks.example.com/v";

    let hook_id = seed(&repository, &tenant, &format!("hook-{run}"), endpoint, None).await;
    let after_create = scope_version(&repository, &tenant).await;

    call(
        &governance,
        update_transport(&hook_id, webhook_transport(endpoint, "")),
        &tenant,
    )
    .await;
    let after_update = scope_version(&repository, &tenant).await;
    assert!(after_update > after_create);

    call(&governance, set_status(&hook_id, false), &tenant).await;
    let after_disable = scope_version(&repository, &tenant).await;
    assert!(after_disable > after_update);

    // 状态没变不算写入。
    call(&governance, set_status(&hook_id, false), &tenant).await;
    assert_eq!(scope_version(&repository, &tenant).await, after_disable);

    call(&governance, delete(&hook_id), &tenant).await;
    assert!(scope_version(&repository, &tenant).await > after_disable);
}

/// 点位只有发送前，保留头不能写，`local` 不是库里的目标；被拒的请求什么也没写。
#[tokio::test]
#[ignore = "requires FLARE_TENANT_TEST_DATABASE_URL pointing at a database initialised with deploy/init.sql"]
async fn administer_rejects_removed_hook_types_and_reserved_headers() {
    let repository = repository().await;
    let run = suffix();
    let governance = HookServiceServer::new(Arc::clone(&repository));
    let typed = |hook_type: &str| {
        administer(
            "create",
            "CreateHookConfigRequest",
            CreateHookConfigRequest {
                name: format!("typed-{hook_type}-{run}"),
                hook_type: hook_type.into(),
                order: 900,
                transport: Some(webhook_transport("https://hooks.example.com/t", "")),
                ..Default::default()
            }
            .encode_to_vec(),
        )
    };
    for hook_type in ["post_send", "delivery", "recall", "message_read", "custom"] {
        let status = dispatch_hook_administer(&governance, typed(hook_type), &platform_employee())
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument, "{status:?}");
        assert!(status.message().contains("unknown hook_type"), "{status:?}");
    }

    let mut reserved = webhook_transport("https://hooks.example.com/t", "");
    reserved
        .headers
        .insert("Flare-Signature".into(), "header-value".into());
    let status = dispatch_hook_administer(
        &governance,
        create("", &format!("reserved-{run}"), reserved),
        &platform_employee(),
    )
    .await
    .unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument, "{status:?}");
    assert!(status.message().contains("is reserved"), "{status:?}");
    assert!(!status.message().contains("header-value"), "{status:?}");

    let local = HookTransport {
        r#type: "local".into(),
        timeout_ms: 1000,
        ..Default::default()
    };
    let status = dispatch_hook_administer(
        &governance,
        create("", &format!("local-{run}"), local),
        &platform_employee(),
    )
    .await
    .unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument, "{status:?}");

    let rows = repository.query(None, None, false).await.unwrap();
    assert!(
        rows.iter().all(|row| !row.name.contains(&run)),
        "a rejected hook was written"
    );
}

/// 平台 Hook：用平台写码建，按请求写 `order` 和 `required`，tenant_id 是 NULL，版本记在空串范围；应用
/// 的员工读不到、改不了；列表里各看各的。
#[tokio::test]
#[ignore = "requires FLARE_TENANT_TEST_DATABASE_URL pointing at a database initialised with deploy/init.sql"]
async fn platform_hooks_are_written_with_the_platform_code() {
    let repository = repository().await;
    let run = suffix();
    let tenant = format!("tp{run}");
    let _cleanup = cleanup(&tenant);
    let governance = HookServiceServer::new(Arc::clone(&repository));
    let secret = format!("platform-secret-{run}");
    let request = administer(
        "create",
        "CreateHookConfigRequest",
        CreateHookConfigRequest {
            name: format!("platform-{run}"),
            hook_type: "pre_send".into(),
            order: 7,
            required: false,
            transport: Some(webhook_transport("https://hooks.example.com/p", &secret)),
            ..Default::default()
        }
        .encode_to_vec(),
    );
    let created = dispatch_hook_administer(&governance, request, &platform_employee())
        .await
        .unwrap()
        .into_inner();
    let config =
        CreateHookConfigResponse::decode(created.payload.as_ref().unwrap().value.as_slice())
            .unwrap()
            .config
            .unwrap();
    assert_write_only(&created, &config, &[secret.as_str()]);
    assert!(config.tenant_id.is_empty());
    assert_eq!(config.order, 7);
    assert!(!config.required);
    let hook_id = config.hook_id;
    let (row, item) = repository
        .get_by_id(hook_id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.tenant_id, None);
    assert!(!item.require_success);
    assert!(scope_version(&repository, "").await > 0);

    for request in [get(&hook_id), set_status(&hook_id, false), delete(&hook_id)] {
        let status = dispatch_hook_administer(&governance, request, &employee(&tenant))
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status:?}");
    }
    let listed = call(&governance, list(&tenant), &tenant).await;
    let configs =
        ListHookConfigsResponse::decode(listed.payload.as_ref().unwrap().value.as_slice())
            .unwrap()
            .configs;
    assert!(configs.iter().all(|config| config.hook_id != hook_id));

    let deleted = dispatch_hook_administer(&governance, delete(&hook_id), &platform_employee())
        .await
        .unwrap()
        .into_inner();
    assert!(deleted.ok);
}

/// 应用 Hook 要出站校验和出网开关，这些没有之前建不出来，什么也不写。
#[tokio::test]
#[ignore = "requires FLARE_TENANT_TEST_DATABASE_URL pointing at a database initialised with deploy/init.sql"]
async fn application_hooks_cannot_be_created_yet() {
    let repository = repository().await;
    let run = suffix();
    let tenant = format!("ta{run}");
    let _cleanup = cleanup(&tenant);
    let governance = HookServiceServer::new(Arc::clone(&repository));
    let status = dispatch_hook_administer(
        &governance,
        create(
            &tenant,
            &format!("hook-{run}"),
            webhook_transport("https://hooks.example.com/a", ""),
        ),
        &employee(&tenant),
    )
    .await
    .unwrap_err();
    assert_eq!(status.code(), tonic::Code::FailedPrecondition, "{status:?}");
    assert!(status.message().contains("application hooks"), "{status:?}");
    assert!(
        repository
            .query(Some(tenant.as_str()), None, false)
            .await
            .unwrap()
            .is_empty()
    );
}

/// 应用不能占用平台行（停用的也算）和部署摘要里的 Hook 的名字。
#[tokio::test]
#[ignore = "requires FLARE_TENANT_TEST_DATABASE_URL pointing at a database initialised with deploy/init.sql"]
async fn applications_cannot_take_platform_or_deployment_names() {
    let repository = repository().await;
    let pool = repository.connection_pool();
    let run = suffix();
    let tenant = format!("tn{run}");
    let _cleanup = cleanup(&tenant);
    let governance = HookServiceServer::new(Arc::clone(&repository));
    let platform_name = format!("platform-{run}");
    let deployment_name = format!("deployment-{run}");

    sqlx::query(
        "INSERT INTO hook_configs (tenant_id, hook_type, name, enabled, transport_config)
         VALUES (NULL, 'pre_send', $1, FALSE, '{\"type\":\"webhook\",\"endpoint\":\"https://e\"}')",
    )
    .bind(&platform_name)
    .execute(pool.as_ref())
    .await
    .unwrap();
    sqlx::query("INSERT INTO hook_deployment_summary (service, hooks) VALUES ($1, $2)")
        .bind(&tenant)
        .bind(serde_json::json!([{"hook_type": "pre_send", "name": deployment_name}]))
        .execute(pool.as_ref())
        .await
        .unwrap();

    let hook_id = seed(
        &repository,
        &tenant,
        &format!("own-{run}"),
        "https://hooks.example.com/n",
        None,
    )
    .await;
    for name in [&platform_name, &deployment_name] {
        let request = administer(
            "update",
            "UpdateHookConfigRequest",
            UpdateHookConfigRequest {
                hook_id: hook_id.clone(),
                name: name.clone(),
                ..Default::default()
            }
            .encode_to_vec(),
        );
        let status = dispatch_hook_administer(&governance, request, &employee(&tenant))
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::AlreadyExists, "{status:?}");
    }

    sqlx::query("DELETE FROM hook_configs WHERE tenant_id IS NULL AND name = $1")
        .bind(&platform_name)
        .execute(pool.as_ref())
        .await
        .unwrap();
}
