//! Signed production gRPC + real owned PostgreSQL + bounded in-memory object bytes.
//! Does not claim S3, browser CORS, public Management or deployed acceptance.
use super::*;
use crate::domain::service::isolation_tests::Sessions;
use crate::domain::{
    model::{MediaOwnerKind, MediaPurpose, ObjectStat, UploadContext},
    repository::{MediaObjectRepository, UploadSessionStore},
};
use flare_grpc_proto::media::*;
use std::sync::Mutex;

#[derive(Default)]
struct Objects(Mutex<HashMap<String, Vec<u8>>>);
#[async_trait::async_trait]
impl MediaObjectRepository for Objects {
    async fn put_validated_object(&self, key: &str, _: &str, bytes: &[u8]) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), bytes.to_vec());
        Ok(())
    }
    async fn put_object(&self, _: &UploadContext<'_>) -> Result<String> {
        unreachable!("Staff completion uses the server-only final write")
    }
    async fn read_object_bounded(&self, key: &str, limit: u64) -> Result<Vec<u8>> {
        let bytes = self.0.lock().unwrap().get(key).unwrap().clone();
        if bytes.len() as u64 > limit {
            return Err(flare_server_core::flare_err!(
                ErrorCode::InvalidParameter,
                "image too large"
            ));
        }
        Ok(bytes)
    }
    async fn delete_object(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
    async fn presign_object(&self, _: &str, _: i64) -> Result<String> {
        Ok("https://objects.invalid/image?signature=temporary".into())
    }
    async fn presign_put_object(&self, _: &str, _: &str, _: i64, _: i64) -> Result<String> {
        Ok("https://objects.invalid/staging?signature=put".into())
    }
    async fn stat_object(&self, key: &str) -> Result<Option<ObjectStat>> {
        Ok(self.0.lock().unwrap().get(key).map(|bytes| ObjectStat {
            size: Some(bytes.len() as i64),
            etag: None,
        }))
    }
    fn base_url(&self) -> Option<String> {
        None
    }
    fn cdn_base_url(&self) -> Option<String> {
        None
    }
    fn use_presigned_urls(&self) -> bool {
        true
    }
    fn bucket_name(&self) -> Option<String> {
        Some("private-images".into())
    }
}
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(width, height)
        .write_to(&mut bytes, image::ImageOutputFormat::Png)
        .unwrap();
    bytes.into_inner()
}
fn staff<T>(payload: T, app: &str, actor: &str) -> Request<T> {
    signed(
        payload,
        app,
        SignedActorType::Staff,
        actor,
        "core:medias:write",
    )
}
fn init(bytes: &[u8], mime: &str) -> AdminInitiateDirectUploadRequest {
    AdminInitiateDirectUploadRequest {
        purpose: 1,
        file_name: "avatar.png".into(),
        mime_type: mime.into(),
        file_size: bytes.len() as i64,
    }
}

#[tokio::test]
#[ignore = "requires explicit owned-test FLARE_FUNCTIONAL_PG_ADMIN_URL"]
async fn application_image_upload_signed_authority_validation_sealing_and_rollback() {
    database(check).await;
}
async fn check(pool: Arc<PgPool>, root: PathBuf) {
    let store = Arc::new(PostgresMetadataStore { pool: pool.clone() });
    let objects = Arc::new(Objects::default());
    let sessions = Arc::new(Sessions::default());
    let mut config =
        MediaDomainConfig::new(3600, None, 60, root.join("chunks"), 3600, 8 * 1024 * 1024);
    config.admin_image_upload.max_dimension = 4;
    config.admin_image_upload.max_pixels = 8;
    config.application_media_validators = vec!["validator".into()];
    let domain = Arc::new(MediaService::new(
        Some(objects.clone()),
        Some(store.clone()),
        None,
        None,
        Some(sessions.clone()),
        None,
        config,
    ));
    let handler = MediaGrpcHandler::new(
        Arc::new(MediaCommandHandler::new(domain.clone())),
        Arc::new(MediaQueryHandler::new(domain)),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let running = Running(tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(authenticated_media_service(handler, keys().1))
            .serve_with_incoming(tonic::transport::server::TcpIncoming::from(listener))
            .await
            .unwrap();
    }));
    let mut client = MediaServiceClient::new(
        Channel::from_shared(format!("http://{address}"))
            .unwrap()
            .connect_lazy(),
    );
    let bytes = png(2, 2);
    assert_eq!(
        client
            .admin_initiate_direct_upload(signed(
                init(&bytes, "image/png"),
                "app-a",
                SignedActorType::User,
                "employee",
                "core:medias:write"
            ))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    let session = client
        .admin_initiate_direct_upload(staff(init(&bytes, "image/png"), "app-a", "employee"))
        .await
        .unwrap()
        .into_inner();
    for (app, actor) in [("app-b", "employee"), ("app-a", "other-employee")] {
        assert!(
            client
                .complete_direct_upload(staff(
                    CompleteDirectUploadRequest {
                        upload_id: session.upload_id.clone()
                    },
                    app,
                    actor
                ))
                .await
                .is_err()
        );
        assert!(!present(&store, "app-a", &session.file_id).await);
    }
    objects
        .0
        .lock()
        .unwrap()
        .insert(session.object_key.clone(), bytes.clone());
    let result = client
        .complete_direct_upload(staff(
            CompleteDirectUploadRequest {
                upload_id: session.upload_id.clone(),
            },
            "app-a",
            "employee",
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.file_id, session.file_id);
    assert!(result.url.is_empty() && result.cdn_url.is_empty());
    let asset = store
        .load_metadata(&Context::root().with_tenant_id("app-a"), &session.file_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (asset.owner_kind, asset.purpose),
        (MediaOwnerKind::Application, MediaPurpose::Avatar)
    );
    assert_eq!(asset.owner_id.as_deref(), Some("app-a"));
    assert!(!present(&store, "app-b", &session.file_id).await);
    objects
        .0
        .lock()
        .unwrap()
        .insert(session.object_key, b"later browser overwrite".to_vec());
    assert_eq!(
        objects
            .0
            .lock()
            .unwrap()
            .get(asset.storage_path.as_ref().unwrap())
            .unwrap(),
        &bytes
    );
    client
        .validate_application_media(signed(
            ValidateApplicationMediaRequest {
                file_id: session.file_id.clone(),
                purpose: 1,
            },
            "app-a",
            SignedActorType::Service,
            "validator",
            "",
        ))
        .await
        .unwrap();
    assert_eq!(
        client
            .validate_application_media(signed(
                ValidateApplicationMediaRequest {
                    file_id: session.file_id,
                    purpose: 1
                },
                "app-b",
                SignedActorType::Service,
                "validator",
                ""
            ))
            .await
            .unwrap_err()
            .code(),
        Code::NotFound
    );
    assert_eq!(effects(&pool).await.len(), 1);
    // Real declared MIME, dimensions and total pixel budget each fail before registration.
    for (bytes, mime) in [
        (png(2, 2), "image/jpeg"),
        (png(5, 1), "image/png"),
        (png(4, 3), "image/png"),
        (b"not an image".to_vec(), "image/png"),
    ] {
        let session = client
            .admin_initiate_direct_upload(staff(init(&bytes, mime), "app-a", "employee"))
            .await
            .unwrap()
            .into_inner();
        objects
            .0
            .lock()
            .unwrap()
            .insert(session.object_key.clone(), bytes);
        assert_eq!(
            client
                .complete_direct_upload(staff(
                    CompleteDirectUploadRequest {
                        upload_id: session.upload_id.clone()
                    },
                    "app-a",
                    "employee"
                ))
                .await
                .unwrap_err()
                .code(),
            Code::InvalidArgument
        );
        assert!(!present(&store, "app-a", &session.file_id).await);
        client
            .abort_direct_upload(staff(
                AbortDirectUploadRequest {
                    upload_id: session.upload_id,
                },
                "app-a",
                "employee",
            ))
            .await
            .unwrap();
        assert!(!objects.0.lock().unwrap().contains_key(&session.object_key));
    }
    assert_eq!(effects(&pool).await.len(), 1);
    // A failed audit INSERT rolls back registration; abort cleans both staged and final bytes.
    sqlx::query(
        "ALTER TABLE core_audit_outbox ADD CONSTRAINT reject_upload_audit CHECK (false) NOT VALID",
    )
    .execute(&*pool)
    .await
    .unwrap();
    let session = client
        .admin_initiate_direct_upload(staff(init(&bytes, "image/png"), "app-a", "employee"))
        .await
        .unwrap()
        .into_inner();
    objects
        .0
        .lock()
        .unwrap()
        .insert(session.object_key.clone(), bytes);
    assert!(
        client
            .complete_direct_upload(staff(
                CompleteDirectUploadRequest {
                    upload_id: session.upload_id.clone()
                },
                "app-a",
                "employee"
            ))
            .await
            .is_err()
    );
    assert!(!present(&store, "app-a", &session.file_id).await);
    assert!(
        sessions
            .get_session(&session.upload_id)
            .await
            .unwrap()
            .is_some()
    );
    client
        .abort_direct_upload(staff(
            AbortDirectUploadRequest {
                upload_id: session.upload_id,
            },
            "app-a",
            "employee",
        ))
        .await
        .unwrap();
    let final_key = session.object_key.strip_prefix("staging/").unwrap();
    assert!(!objects.0.lock().unwrap().contains_key(final_key));
    assert!(!objects.0.lock().unwrap().contains_key(&session.object_key));
    assert_eq!(effects(&pool).await.len(), 1);
    drop(running);
}
