//! R3-2 新增的 Capability 路由契约：能力目录、租户能力开关、插件登记与注销、Hook 治理。
//! 每条路由的缺上下文 401、权限码不符 403、写操作审计头 403，平台专属路由的组织员工 403，
//! 以及经回环替身验证的成功响应形状与下游请求（租户只取自签名）。
mod common;

use axum::http::StatusCode;
use common::*;

fn reason(body: &serde_json::Value) -> &str {
    body["reason"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn every_capability_route_requires_the_signed_route_permission() {
    let h = harness(true).await;
    for (method, path, body) in capability_routes() {
        let extra = if is_write(method, path) {
            WRITE_HEADERS
        } else {
            &[]
        };
        let (status, answer) = send(
            &h.router,
            request_with_body(method, path, None, extra, body),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path}: {answer}"
        );
        let other = h.gateway.staff_token(EMPLOYEE, TENANT, "core:gateway:read");
        let (status, answer) = send(
            &h.router,
            request_with_body(method, path, Some(&other), extra, body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {answer}");
        assert_eq!(
            reason(&answer),
            "ADMIN_PERMISSION_DENIED",
            "{method} {path}"
        );
        if is_write(method, path) {
            let token = h.gateway.token_for(method, path);
            let (status, answer) = send(
                &h.router,
                request_with_body(method, path, Some(&token), &[], body),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {answer}");
            assert_eq!(reason(&answer), "FORBIDDEN", "{method} {path}");
        }
    }
    assert!(h.recorder.calls().is_empty(), "{:?}", h.recorder.calls());

    for (method, path, body) in capability_routes() {
        let (status, answer) = send(
            &h.router,
            admin_request_with_body(method, path, &h.gateway, body),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{method} {path}: {answer}");
    }
    let calls = h.recorder.calls();
    assert_eq!(calls.len(), capability_routes().len());
    for (call, (method, path, _)) in calls.iter().zip(capability_routes()) {
        assert_forwarded_for_route(call, &h.gateway, method, path);
    }
}

#[tokio::test]
async fn capability_catalog_shape() {
    let h = harness(true).await;
    let (status, body) = send(
        &h.router,
        admin_request("GET", "/api/v1/admin/capabilities/catalog", &h.gateway),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(keys(data), sorted(&["capabilities"]));
    let ids: Vec<_> = data["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["capability_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, ["ai.summary", "example.poll"]);
    assert_eq!(
        keys(&data["capabilities"][0]),
        sorted(&[
            "capability_id",
            "plugin_id",
            "version",
            "scope",
            "visibility",
            "permissions",
            "message_types",
            "timeout_ms",
            "description",
        ])
    );
    assert_eq!(h.recorder.calls()[0].method, "ListCapabilities");
}

#[tokio::test]
async fn tenant_capability_switch_targets_the_signed_application() {
    let h = harness(true).await;
    let (status, body) = send(
        &h.router,
        admin_request_with_body(
            "PUT",
            "/api/v1/admin/capabilities/tenant/example.poll",
            &h.gateway,
            Some(r#"{"enabled":false}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(
        keys(data),
        sorted(&["tenant_id", "capability_id", "enabled", "message"])
    );
    assert_eq!(data["tenant_id"], TENANT);
    assert_eq!(data["capability_id"], "example.poll");
    assert_eq!(data["enabled"], false);
    let call = &h.recorder.calls()[0];
    assert_eq!(call.method, "SetTenantCapabilitySwitch");
    assert_eq!(
        call.request,
        format!("tenant_id={TENANT} capability_id=example.poll enabled=false")
    );

    // 另一应用的员工：租户只取自签名。
    h.recorder.clear();
    let other = h.gateway.token_for_tenant(
        "PUT",
        "/api/v1/admin/capabilities/tenant/example.poll",
        OTHER_TENANT,
    );
    let (status, _) = send(
        &h.router,
        request_with_body(
            "PUT",
            "/api/v1/admin/capabilities/tenant/example.poll",
            Some(&other),
            WRITE_HEADERS,
            Some(r#"{"enabled":true}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.recorder.calls()[0].request,
        format!("tenant_id={OTHER_TENANT} capability_id=example.poll enabled=true")
    );

    // 下游拒绝与请求体错误。
    for (capability, expected) in [
        ("denied", StatusCode::FORBIDDEN),
        ("down", StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let path = format!("/api/v1/admin/capabilities/tenant/{capability}");
        let (status, body) = send(
            &h.router,
            admin_request_with_body("PUT", &path, &h.gateway, Some(r#"{"enabled":true}"#)),
        )
        .await;
        assert_eq!(status, expected, "{capability}: {body}");
        assert!(!body.to_string().contains("id-secret-1"), "{body}");
    }
    let (status, body) = send(
        &h.router,
        admin_request_with_body(
            "PUT",
            "/api/v1/admin/capabilities/tenant/example.poll",
            &h.gateway,
            Some(r#"{"enabled":"leak-marker-77"}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(reason(&body), "ADMIN_INVALID_JSON_BODY");
    assert!(!body.to_string().contains("leak-marker-77"));
}

const HOOK_FIELDS: &[&str] = &[
    "hook_id",
    "name",
    "hook_type",
    "tenant_id",
    "order",
    "required",
    "enabled",
    "transport",
    "selector",
    "created_at",
    "updated_at",
];

#[tokio::test]
async fn hook_list_create_get_update_shapes_and_requests() {
    let h = harness(true).await;
    let (status, body) = send(
        &h.router,
        admin_request(
            "GET",
            "/api/v1/admin/hooks?hook_type=message_pre_send&enabled_only=true&limit=20",
            &h.gateway,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(keys(data), sorted(&["hooks", "limit", "has_more", "total"]));
    assert_eq!(data["limit"], 20);
    assert_eq!(data["total"], 2);
    let hook = &data["hooks"][0];
    assert_eq!(keys(hook), sorted(HOOK_FIELDS));
    assert_eq!(hook["order"], 10);
    assert_eq!(hook["required"], true);
    assert_eq!(hook["created_at"], 1_700_000_000_000_i64);
    // webhook 密钥不出网关。
    assert_eq!(hook["transport"]["secret_configured"], true);
    assert!(hook["transport"].get("secret").is_none(), "{hook}");
    assert!(!body.to_string().contains("webhook-secret"), "{body}");
    let call = &h.recorder.calls()[0];
    assert_eq!(call.method, "Administer");
    assert_eq!(
        call.request,
        format!("list tenant_id={TENANT} hook_type=message_pre_send enabled_only=true limit=20")
    );

    // 应用 Hook 建不出来：给明确的错误，不到 Capability。
    h.recorder.clear();
    let (status, body) = send(
        &h.router,
        admin_request_with_body(
            "POST",
            "/api/v1/admin/hooks",
            &h.gateway,
            Some(r#"{"name":"dlp","hook_type":"pre_send"}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(reason(&body), "ADMIN_HOOK_APPLICATION_UNAVAILABLE");
    assert!(h.recorder.calls().is_empty());

    // 创建平台 Hook：范围来自路由，请求体里的 tenant_id 忽略；密钥只进不出。
    h.recorder.clear();
    let (status, body) = send(
        &h.router,
        admin_request_with_body(
            "POST",
            "/api/v1/admin/platform/hooks",
            &h.gateway,
            Some(
                r#"{"name":"dlp","hook_type":"pre_send","tenant_id":"tenant-z","order":10,"required":false,"transport":{"type":"webhook","endpoint":"https://hooks.example/dlp","secret":"s3cret"}}"#,
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(keys(&body["data"]), sorted(&["hook"]));
    assert_eq!(body["data"]["hook"]["hook_id"], "9");
    assert_eq!(body["data"]["hook"]["name"], "dlp");
    assert!(!body.to_string().contains("s3cret"), "{body}");
    assert_eq!(
        h.recorder.calls()[0].request,
        "create tenant_id= name=dlp hook_type=pre_send order=10 required=false secret_set=true"
    );
    for (invalid, expected_reason) in [
        (
            r#"{"name":"","hook_type":"pre_send"}"#,
            "ADMIN_HOOK_NAME_REQUIRED",
        ),
        (r#"{"name":"dlp"}"#, "ADMIN_INVALID_JSON_BODY"),
    ] {
        let (status, body) = send(
            &h.router,
            admin_request_with_body(
                "POST",
                "/api/v1/admin/platform/hooks",
                &h.gateway,
                Some(invalid),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}: {body}");
        assert_eq!(reason(&body), expected_reason, "{invalid}");
    }

    h.recorder.clear();
    let (status, body) = send(
        &h.router,
        admin_request("GET", "/api/v1/admin/hooks/7", &h.gateway),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["hook"]["hook_id"], "7");
    assert_eq!(h.recorder.calls()[0].request, "get hook_id=7");
    // `hook_type:name` 形式的 id 也是合法参数。
    let (status, _) = send(
        &h.router,
        admin_request(
            "GET",
            "/api/v1/admin/hooks/message_pre_send:dlp",
            &h.gateway,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    h.recorder.clear();
    let (status, body) = send(
        &h.router,
        admin_request_with_body(
            "PUT",
            "/api/v1/admin/hooks/7",
            &h.gateway,
            Some(r#"{"name":"dlp-2","order":20,"required":false}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["hook"]["name"], "dlp-2");
    assert_eq!(
        h.recorder.calls()[0].request,
        "update hook_id=7 name=dlp-2 order=Some(20) required=Some(false)"
    );
}

#[tokio::test]
async fn platform_hook_catalog_profiles_and_test_call() {
    let h = harness(true).await;
    let (status, body) = send(
        &h.router,
        admin_request("GET", "/api/v1/admin/platform/hooks?limit=20", &h.gateway),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        h.recorder.calls()[0].request,
        "list tenant_id= hook_type= enabled_only=false limit=20"
    );

    let (status, body) = send(
        &h.router,
        admin_request("GET", "/api/v1/admin/platform/hooks/points", &h.gateway),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["points"][0]["id"], "message.pre_send");
    assert_eq!(body["data"]["points"][0]["hook_type"], "pre_send");

    let (status, body) = send(
        &h.router,
        admin_request("GET", "/api/v1/admin/platform/hooks/profiles", &h.gateway),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let profile = &body["data"]["profiles"][0];
    assert_eq!(profile["service"], "message-ingest");
    assert_eq!(profile["hooks"][0]["name"], "social");
    assert_eq!(profile["hooks"][0]["target_type"], "grpc");

    h.recorder.clear();
    let (status, body) = send(
        &h.router,
        admin_request_with_body(
            "POST",
            "/api/v1/admin/platform/hooks/test",
            &h.gateway,
            Some(
                r#"{"hook_type":"pre_send","transport":{"type":"webhook","endpoint":"https://hooks.example/t","timeout_ms":500}}"#,
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["outcome"], "allowed");
    assert_eq!(body["data"]["latency_ms"], 12);
    assert_eq!(
        h.recorder.calls()[0].request,
        "test hook_id= endpoint=https://hooks.example/t"
    );

    let (status, body) = send(
        &h.router,
        admin_request_with_body(
            "POST",
            "/api/v1/admin/platform/hooks/test",
            &h.gateway,
            Some("{}"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(reason(&body), "ADMIN_HOOK_TARGET_REQUIRED");
}

#[tokio::test]
async fn hook_delete_status_statistics_and_executions() {
    let h = harness(true).await;
    let (status, body) = send(
        &h.router,
        admin_request("DELETE", "/api/v1/admin/hooks/7", &h.gateway),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(keys(&body["data"]), sorted(&["hook_id", "deleted"]));
    assert_eq!(body["data"]["deleted"], true);
    assert_eq!(h.recorder.calls()[0].request, "delete hook_id=7");

    h.recorder.clear();
    let (status, body) = send(
        &h.router,
        admin_request_with_body(
            "PUT",
            "/api/v1/admin/hooks/7/status",
            &h.gateway,
            Some(r#"{"enabled":false}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        keys(&body["data"]),
        sorted(&["hook_id", "enabled", "updated"])
    );
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(
        h.recorder.calls()[0].request,
        "set_status hook_id=7 enabled=false"
    );

    h.recorder.clear();
    let (status, body) = send(
        &h.router,
        admin_request(
            "GET",
            "/api/v1/admin/hooks/7/statistics?start_time=1700000000000&end_time=1700003600000",
            &h.gateway,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        keys(&body["data"]),
        sorted(&[
            "hook_id",
            "total_executions",
            "success_count",
            "failure_count",
            "avg_latency_ms",
            "p99_latency_ms",
            "rate_limit_count",
            "circuit_break_count",
            "error_count_by_code",
        ])
    );
    assert_eq!(body["data"]["success_count"], 9);
    assert_eq!(
        h.recorder.calls()[0].request,
        "get_statistics hook_id=7 start=Some(1700000000000) end=Some(1700003600000)"
    );
    let (status, body) = send(
        &h.router,
        admin_request(
            "GET",
            "/api/v1/admin/hooks/7/statistics?start_time=20&end_time=10",
            &h.gateway,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(reason(&body), "ADMIN_HOOK_TIME_RANGE_INVALID");

    h.recorder.clear();
    let (status, body) = send(
        &h.router,
        admin_request(
            "GET",
            "/api/v1/admin/hooks/7/executions?message_id=m1&success_only=true&limit=5",
            &h.gateway,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        keys(&body["data"]),
        sorted(&["hook_id", "executions", "limit", "has_more"])
    );
    assert_eq!(body["data"]["limit"], 5);
    assert_eq!(
        keys(&body["data"]["executions"][0]),
        sorted(&[
            "execution_id",
            "hook_id",
            "message_id",
            "success",
            "latency_ms",
            "error_code",
            "error_message",
            "executed_at",
        ])
    );
    assert_eq!(
        body["data"]["executions"][0]["executed_at"],
        1_700_000_000_000_i64
    );
    assert_eq!(
        h.recorder.calls()[0].request,
        "query_executions hook_id=7 message_id=m1 success_only=true limit=5"
    );

    // Capability 的拒绝：不存在 404、别的应用的配置 403，都不回显文案。
    for (hook, expected, expected_reason) in [
        ("hook-missing", StatusCode::NOT_FOUND, "NOT_FOUND"),
        ("hook-foreign", StatusCode::FORBIDDEN, "FORBIDDEN"),
    ] {
        for (method, suffix, body) in [
            ("GET", "", None),
            ("PUT", "", Some(r#"{"name":"x"}"#)),
            ("DELETE", "", None),
            ("PUT", "/status", Some(r#"{"enabled":true}"#)),
            ("GET", "/statistics", None),
            ("GET", "/executions", None),
        ] {
            let path = format!("/api/v1/admin/hooks/{hook}{suffix}");
            let (status, answer) = send(
                &h.router,
                admin_request_with_body(method, &path, &h.gateway, body),
            )
            .await;
            assert_eq!(status, expected, "{method} {path}: {answer}");
            assert_eq!(reason(&answer), expected_reason, "{method} {path}");
            assert!(!answer.to_string().contains("id-secret-1"), "{answer}");
        }
    }
}
