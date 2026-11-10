-- Keep authenticated User ownership for every historical asset. Application
-- ownership and image purpose are established only by the Staff upload path.
ALTER TABLE media_assets
    ADD COLUMN owner_kind TEXT NOT NULL DEFAULT 'user'
        CHECK (owner_kind IN ('user', 'application')),
    ADD COLUMN purpose TEXT NOT NULL DEFAULT 'general'
        CHECK (purpose IN ('general', 'avatar', 'image'));

ALTER TABLE media_assets ADD CONSTRAINT media_assets_application_owner
    CHECK (owner_kind <> 'application' OR (owner_id IS NOT NULL AND owner_id = tenant_id));

COMMENT ON COLUMN media_assets.owner_kind IS 'Authenticated User owner or explicitly application-owned managed asset';
COMMENT ON COLUMN media_assets.purpose IS 'Authoritative media purpose, never read from client metadata';
