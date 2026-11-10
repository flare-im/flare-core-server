//! `PluginDeployment`: the operator's shared service for one version. Its binding is immutable;
//! only its health changes.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDeploymentRecord {
    pub deployment_id: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub manifest_digest: String,
    pub artifact_digest: String,
    pub grpc_authority: String,
    pub healthy_until_ms: i64,
    pub last_error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentRegistration {
    pub deployment_id: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub grpc_authority: String,
}

impl DeploymentRegistration {
    pub fn valid_id(&self) -> bool {
        valid_deployment_id(&self.deployment_id)
    }
}

pub fn valid_deployment_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentFilter {
    pub deployment_id: Option<String>,
    pub plugin_id: Option<String>,
    pub plugin_version: Option<String>,
    pub manifest_digest: Option<String>,
    pub limit: Option<i64>,
}
