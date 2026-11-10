-- 开发模式：flare-plugin dev 把本机进程接到标记为开发用途的应用上。
-- 开发版本未签名，和正式版本同表是为了复用部署、应用状态和运行时收敛；
-- 所有正式路径都排除它，见 domain/plugin/development.rs。
ALTER TABLE plugin_versions ADD COLUMN development boolean NOT NULL DEFAULT false;
ALTER TABLE plugin_versions ADD CONSTRAINT plugin_versions_development_unpublished CHECK(NOT (development AND published));
CREATE TABLE plugin_dev_apps (
 app_id text PRIMARY KEY CHECK(app_id ~ '^[0-9A-Za-z_-]{1,32}$'),
 created_at bigint NOT NULL DEFAULT (extract(epoch FROM clock_timestamp())*1000)::bigint
);
