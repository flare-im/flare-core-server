# Application image upload

Core Admin Gateway owns the Staff image upload API under `/api/v1/admin/medias`.
The public Management API maps it under `/admin/v1/apps/{app_id}/medias`.
All tenant and actor identities come from the verified Staff assertion. The request
never accepts an IM user identity, owner, bucket, storage path or upload origin.

1. `GET /upload-policy?purpose=avatar` (or `image`) returns the authoritative
   `max_bytes`, `max_pixels`, `max_dimension` and `allowed_mime_types`.
2. `POST /uploads/initiate` accepts `purpose`, `file_name`, `mime_type`, `file_size`.
   It returns the bound tenant, session/file IDs, transport, part size/count,
   presigned storage upload URL, upload headers and session expiry.
3. PUT the file to that signed URL using only the returned upload headers, with
   browser credentials omitted. Never send Staff Authorization or cookies to storage.
   Multipart sessions use `/uploads/presign-parts` and `/uploads/commit-parts`.
4. `POST /uploads/complete` with `upload_id` returns durable `file_id`, `tenant_id`,
   `purpose`, `mime_type`, `size`. Persist the ID as a media reference.
   `/uploads/abort` cancels a session; `GET /uploads/{upload_id}` queries its state.

Policy/session operations require `core:medias:write`. Continuations additionally
require the same signed application and Staff actor that initiated the session.
`GET /files/{file_id}/url` requires `core:medias:read` and returns short-lived
preview access. A URL is never a persistent media identifier.

Completion checks the object's HEAD size and reads at most `max_bytes + 1` bytes.
The server verifies magic bytes, MIME, dimensions, pixel budget and bounded image
decode. For animation, `max_pixels` also bounds cumulative frame canvases before
decode; each GIF frame is decoded and checked. The browser writes a staging key; only the server writes the validated
bytes to the final key. Reusing an old PUT URL cannot change a registered image.
Metadata records `owner_kind=application`, `owner_id=tenant_id` and typed purpose.
New image registration and its Staff audit outbox fact commit in one transaction.
Historical assets remain User-owned; historical sessions retain their User principal.

The defaults in `[services.media]` are:

```toml
admin_image_upload_max_bytes = 8388608
admin_image_upload_max_pixels = 16777216
admin_image_upload_max_dimension = 8192
admin_image_upload_allowed_mime_types = ["image/png", "image/jpeg", "image/webp", "image/gif"]
application_media_validators = []
```

Validators default to empty (deny all). A deployment may explicitly add its trusted
application service actor, for example `application_media_validators = ["flare-user"]`.
That actor can call only the narrow `ValidateApplicationMedia` and
`ResolveApplicationAvatarMedia` RPCs for Active Application-owned images of the
exact signed tenant and requested purpose. Resolution returns at most 300 seconds
of access. It does not authorize general Service reads of private User assets.
Same-application authenticated IM users can resolve Application-owned avatars
through existing Media URL APIs (at most 3600 seconds); generic `image` assets
remain Staff-only. Clients store `avatar_key`, never a presigned GET URL.

Apply migration `0029_20261109_application_media_ownership.sql` before the new
Media binary. Fresh schema sources carry the same columns and constraint. Upgrade
Media readers/writers together before enabling Staff uploads; an old binary does
not understand the new Staff session principal or Application ownership. Configure
private storage and a lifecycle expiry for abandoned `staging/` prefix objects, including
objects recreated by an unexpired PUT after completion/abort. Deployment, storage
CORS and live policy updates are separate operational steps.

Focused gates (run only in the shared Rust lane): Media image validation tests,
signed gRPC `admin_upload_tests`, historical session decoding, Admin Gateway
`admin_media_upload`, and updated OpenAPI snapshots. The ignored
`application_media_ownership_and_source_audit_commit_atomically` test needs an
explicit owned disposable PostgreSQL instance; it validates repository/audit
atomicity and does not claim real object-store acceptance.
