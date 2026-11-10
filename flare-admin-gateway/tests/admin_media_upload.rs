//! HTTP DTO and exact Staff permission forwarding. Image verification is tested in Media.
mod common;
use axum::http::StatusCode;
use common::{media::*, *};

#[tokio::test]
async fn upload_policy_and_durable_result_are_tenant_scoped_and_typed() {
    let h = media_harness(None).await;
    let (status, body) = send(
        &h.router,
        media_admin_request(
            "GET",
            "/api/v1/admin/medias/upload-policy?purpose=avatar",
            &h.gateway,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["tenant_id"], TENANT);
    assert_eq!(body["data"]["max_bytes"], 8 * 1024 * 1024);
    let (status, body) = send(&h.router, media_admin_request("POST", "/api/v1/admin/medias/uploads/initiate", &h.gateway, Some(r#"{"purpose":"avatar","file_name":"头像.png","mime_type":"image/png","file_size":2048}"#))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["transport_kind"], "single_put");
    assert_eq!(body["data"]["upload_headers"]["Content-Type"], "image/png");
    assert!(body["data"].get("object_key").is_none());
    let (status, body) = send(
        &h.router,
        media_admin_request(
            "POST",
            "/api/v1/admin/medias/uploads/complete",
            &h.gateway,
            Some(r#"{"upload_id":"upload-1"}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["file_id"], "image-1");
    assert_eq!(body["data"]["purpose"], "avatar");
    for transient in ["url", "cdn_url", "object_key", "bucket"] {
        assert!(body["data"].get(transient).is_none());
    }
    for call in h.media.calls() {
        assert_eq!(call.signed_context.len(), 1);
        assert!(call.authorization.is_empty());
    }
}

#[tokio::test]
async fn uploads_reject_untrusted_ownership_before_rpc_and_need_write_permission() {
    let h = media_harness(None).await;
    let path = "/api/v1/admin/medias/uploads/initiate";
    for extra in ["user_id", "tenant_id", "owner_id", "object_key"] {
        let body = format!(
            r#"{{"purpose":"avatar","file_name":"a.png","mime_type":"image/png","file_size":1,"{extra}":"victim"}}"#
        );
        let (status, _) = send(
            &h.router,
            media_admin_request("POST", path, &h.gateway, Some(&body)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let read = h
        .gateway
        .token_for("GET", "/api/v1/admin/medias/files/file-1/url");
    let (status, _) = send(
        &h.router,
        media_request(
            "GET",
            "/api/v1/admin/medias/upload-policy?purpose=avatar",
            Some(&read),
            &[],
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(h.media.calls().is_empty());
}
