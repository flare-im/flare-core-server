-- 插件注册表：版本、命名空间、素材、部署，以及每个应用的期望、配置和事实。
-- 换一个没有市场的部署也需要这些，所以归 Core；上架、申请和绑定留在 Social。
CREATE TABLE plugin_versions (
 plugin_id varchar(128) NOT NULL,
 plugin_version varchar(64) NOT NULL,
 display_name varchar(160) NOT NULL,
 publisher_id varchar(192) NOT NULL,
 -- 发布者签名的原始字节，必须是 text：摘要按字节比较。
 manifest_json text NOT NULL CHECK(octet_length(manifest_json)<=1048576),
 manifest_digest text NOT NULL UNIQUE CHECK(manifest_digest ~ '^sha256:[0-9a-f]{64}$'),
 key_id varchar(128) NOT NULL,
 signature varchar(128) NOT NULL,
 kind text NOT NULL CHECK(kind IN('im','business')),
 readme text,
 changelog text,
 published boolean NOT NULL DEFAULT false,
 revoked boolean NOT NULL DEFAULT false,
 suspended boolean NOT NULL DEFAULT false,
 created_at bigint NOT NULL DEFAULT (extract(epoch FROM clock_timestamp())*1000)::bigint,
 updated_at bigint NOT NULL DEFAULT (extract(epoch FROM clock_timestamp())*1000)::bigint,
 PRIMARY KEY(plugin_id,plugin_version),
 UNIQUE(plugin_id,plugin_version,manifest_digest),
 CHECK(NOT (published AND revoked))
);
CREATE INDEX idx_plugin_versions_updated ON plugin_versions(updated_at,plugin_id,plugin_version);
CREATE TABLE plugin_namespaces (
 namespace varchar(192) PRIMARY KEY,
 plugin_id varchar(128) NOT NULL
);
CREATE TABLE plugin_version_assets (
 plugin_id varchar(128) NOT NULL,
 plugin_version varchar(64) NOT NULL,
 path varchar(256) NOT NULL,
 digest text NOT NULL CHECK(digest ~ '^sha256:[0-9a-f]{64}$'),
 media_type varchar(64) NOT NULL,
 bytes bytea NOT NULL CHECK(octet_length(bytes)<=33554432),
 PRIMARY KEY(plugin_id,plugin_version,path),
 FOREIGN KEY(plugin_id,plugin_version) REFERENCES plugin_versions(plugin_id,plugin_version)
);
CREATE TABLE plugin_deployments (
 deployment_id varchar(128) PRIMARY KEY,
 plugin_id varchar(128) NOT NULL,
 plugin_version varchar(64) NOT NULL,
 manifest_digest text NOT NULL,
 artifact_digest text NOT NULL CHECK(artifact_digest ~ '^sha256:[0-9a-f]{64}$'),
 grpc_authority text NOT NULL CHECK(octet_length(grpc_authority)<=512),
 healthy_until_ms bigint NOT NULL DEFAULT 0,
 last_error text,
 created_at bigint NOT NULL DEFAULT (extract(epoch FROM clock_timestamp())*1000)::bigint,
 FOREIGN KEY(plugin_id,plugin_version,manifest_digest) REFERENCES plugin_versions(plugin_id,plugin_version,manifest_digest)
);
CREATE INDEX idx_plugin_deployments_release ON plugin_deployments(plugin_id,plugin_version,manifest_digest,created_at);
-- binding_id 是 Social 的绑定 ID，也是 grant_id。一个应用的同一插件可以有多行（升级期间两个版本）。
CREATE TABLE plugin_app_states (
 app_id text NOT NULL CHECK(app_id ~ '^[0-9A-Za-z_-]{1,32}$'),
 binding_id varchar(128) NOT NULL,
 plugin_id varchar(128) NOT NULL,
 plugin_version varchar(64) NOT NULL,
 manifest_digest text NOT NULL,
 revision bigint NOT NULL CHECK(revision>0),
 enabled boolean NOT NULL DEFAULT false,
 configured boolean NOT NULL DEFAULT false,
 configuration jsonb NOT NULL DEFAULT '{}'::jsonb CHECK(jsonb_typeof(configuration)='object' AND octet_length(configuration::text)<=32768),
 deployment_id varchar(128) REFERENCES plugin_deployments(deployment_id),
 runtime_revision bigint NOT NULL DEFAULT 0 CHECK(runtime_revision>=0),
 runtime_configuration_digest text,
 runtime_enabled boolean NOT NULL DEFAULT false,
 runtime_healthy_until_ms bigint NOT NULL DEFAULT 0,
 runtime_error text,
 runtime_error_detail text,
 runtime_configuration jsonb CHECK(runtime_configuration IS NULL OR (jsonb_typeof(runtime_configuration)='object' AND octet_length(runtime_configuration::text)<=32768)),
 runtime_attempts integer NOT NULL DEFAULT 0 CHECK(runtime_attempts>=0),
 runtime_next_attempt_ms bigint NOT NULL DEFAULT 0,
 runtime_enabled_digest text,
 -- 以下来自投影：reconcile 要的期望状态。配置走 plugin_config.put，其余都走投影，所以停用不需要 Core 在线。
 organization_id varchar(128) NOT NULL DEFAULT '',
 lifecycle text NOT NULL DEFAULT 'pending' CHECK(lifecycle IN ('pending','installed','uninstalled','revoked','failed')),
 suspended boolean NOT NULL DEFAULT false,
 owned boolean NOT NULL DEFAULT false,
 offline_hold boolean NOT NULL DEFAULT false,
 export_until_ms bigint,
 approved jsonb NOT NULL DEFAULT '{}'::jsonb CHECK(jsonb_typeof(approved)='object'),
 features jsonb NOT NULL DEFAULT '{}'::jsonb CHECK(jsonb_typeof(features)='object'),
 projected_revision bigint NOT NULL DEFAULT 0 CHECK(projected_revision>=0),
 updated_at bigint NOT NULL DEFAULT (extract(epoch FROM clock_timestamp())*1000)::bigint,
 PRIMARY KEY(app_id,binding_id),
 FOREIGN KEY(plugin_id,plugin_version,manifest_digest) REFERENCES plugin_versions(plugin_id,plugin_version,manifest_digest)
);
CREATE INDEX idx_plugin_app_states_deployment ON plugin_app_states(deployment_id);
CREATE INDEX idx_plugin_app_states_reconcile ON plugin_app_states(updated_at,app_id,binding_id);
-- 运行时收敛的轮次：同一时刻只有一个副本在跑。
CREATE TABLE plugin_reconcile_lease (
 id integer PRIMARY KEY CHECK(id=1),
 holder text NOT NULL,
 until_ms bigint NOT NULL
);
-- 注册表变化的水位。scope 空串是全局（版本状态、部署恢复），否则是应用（配置、事实）。
-- 解析缓存按它和投影版本探测，撤权和状态变化下一次读取就生效。
CREATE TABLE plugin_registry_revisions (
 scope text PRIMARY KEY,
 revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0)
);
