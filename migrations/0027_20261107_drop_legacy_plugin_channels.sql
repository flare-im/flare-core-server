-- 插件部署和授权已由 plugin_versions、plugin_deployments、plugin_app_states 表达，
-- 按租户的端点登记和按用户的能力授权没有写入方了。Core 急停仍在 capability_tenant_switches。
DROP TABLE IF EXISTS public.capability_plugin_endpoints;
DROP TABLE IF EXISTS public.capability_user_grants;
