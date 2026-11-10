-- One hook runtime. Message Ingest runs every hook; Capability only writes configuration.
--
-- hook_configs: only pre_send is left, and the target is gRPC or a webhook. Rows of removed points
-- and `local` targets are deleted (nothing was deployed on them). hook_order takes over from
-- priority inside a source; group_name, error_policy, max_retries and version are gone: a hook is
-- required or optional. Platform rows (tenant_id IS NULL) get the uniqueness the table constraint
-- cannot give them.
--
-- hook_config_versions: one row per scope, the application id or '' for platform rows. Capability
-- writes it in the same transaction as the hook, under an advisory lock, so versions follow commit
-- order; Message Ingest polls for scopes newer than the version it holds and reloads only those.
--
-- hook_deployment_summary: what the deployment file installed, one row per service, written by
-- Message Ingest at startup and shown read-only. It carries no secrets, header or metadata values.
DELETE FROM hook_configs
 WHERE hook_type <> 'pre_send'
    OR transport_config->>'type' IS DISTINCT FROM 'grpc'
   AND transport_config->>'type' IS DISTINCT FROM 'webhook';

DELETE FROM hook_configs AS older
 USING hook_configs AS newer
 WHERE older.tenant_id IS NULL
   AND newer.tenant_id IS NULL
   AND older.hook_type = newer.hook_type
   AND older.name = newer.name
   AND older.id < newer.id;

-- The selector holds what the runtime matches on and nothing else; flare_im_hooks reads it strictly.
UPDATE hook_configs
   SET selector_config = (
       SELECT COALESCE(jsonb_object_agg(key, value), '{}'::jsonb)
         FROM jsonb_each(selector_config)
        WHERE key IN ('tenants', 'conversation_types', 'message_types')
   );

ALTER TABLE hook_configs ADD COLUMN hook_order INT NOT NULL DEFAULT 0;
UPDATE hook_configs SET hook_order = LEAST(GREATEST(priority, 0), 1000);
ALTER TABLE hook_configs
    DROP COLUMN version,
    DROP COLUMN priority,
    DROP COLUMN group_name,
    DROP COLUMN max_retries,
    DROP COLUMN error_policy;

CREATE UNIQUE INDEX uq_hook_configs_platform_name
    ON hook_configs(hook_type, name) WHERE tenant_id IS NULL;

CREATE TABLE hook_config_versions (
    scope TEXT PRIMARY KEY,
    version BIGINT NOT NULL CHECK (version > 0),
    changed_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_hook_config_versions_version ON hook_config_versions(version);

CREATE TABLE hook_deployment_summary (
    service TEXT PRIMARY KEY,
    source TEXT,
    hooks JSONB NOT NULL DEFAULT '[]'::jsonb,
    instance_id TEXT,
    loaded_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);
