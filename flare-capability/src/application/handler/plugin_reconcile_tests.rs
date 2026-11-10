//! The worker's own rules; the per-binding rules are in `domain::plugin::reconcile`.
use super::*;

fn deployment() -> PluginDeploymentRecord {
    PluginDeploymentRecord {
        deployment_id: "d1".into(),
        plugin_id: "example.poll".into(),
        plugin_version: "1.0.0".into(),
        manifest_digest: "sha256:m".into(),
        artifact_digest: "sha256:a".into(),
        grpc_authority: "http://127.0.0.1:7902".into(),
        healthy_until_ms: 0,
        last_error: None,
        created_at: 1,
    }
}

fn inspection() -> RuntimeInspection {
    RuntimeInspection {
        deployment_id: "d1".into(),
        plugin_id: "example.poll".into(),
        plugin_version: "1.0.0".into(),
        manifest_digest: "sha256:m".into(),
        artifact_digest: "sha256:a".into(),
        ready: true,
        draining: false,
        protocols: Vec::new(),
    }
}

#[test]
fn a_deployment_is_used_only_as_what_it_was_registered() {
    assert!(admitted(Ok(inspection()), &deployment()).is_ok());
    let mut other = inspection();
    other.artifact_digest = "sha256:b".into();
    assert!(matches!(
        admitted(Ok(other), &deployment()),
        Err(Outcome::Rejected(_))
    ));
}

#[test]
fn a_draining_or_unready_deployment_is_retried_not_refused() {
    let mut draining = inspection();
    draining.draining = true;
    assert_eq!(
        admitted(Ok(draining), &deployment()).unwrap_err(),
        Outcome::Unavailable("runtime_unavailable")
    );
    let mut unready = inspection();
    unready.ready = false;
    assert_eq!(
        admitted(Ok(unready), &deployment()).unwrap_err(),
        Outcome::Unavailable("runtime_unavailable")
    );
}

#[test]
fn a_host_refusal_keeps_its_reason_and_an_unknown_outcome_stays_retryable() {
    assert_eq!(
        failed(RuntimeFailure::Refused("bad\nconfiguration".into())),
        Outcome::Rejected("bad configuration".into())
    );
    assert_eq!(
        failed(RuntimeFailure::Unavailable),
        Outcome::Unavailable("runtime_unavailable")
    );
}

#[test]
fn an_answer_for_another_state_is_not_an_acknowledgement() {
    let delivery = TenantDelivery {
        organization_id: "101".into(),
        deployment_id: "d1".into(),
        app_id: "201".into(),
        grant_id: "b1".into(),
        revision: 3,
        manifest_digest: "sha256:m".into(),
        configuration: serde_json::json!({}),
        configuration_digest: "sha256:c".into(),
        enabled: true,
        valid_until_ms: 1,
        validate_configuration: true,
        approved_operations: Vec::new(),
        permissions: Vec::new(),
        data_scopes: Vec::new(),
    };
    let ack = |revision, enabled| RuntimeAck {
        revision,
        configuration_digest: "sha256:c".into(),
        enabled,
    };
    assert!(matches_delivery(&ack(3, true), &delivery));
    assert!(!matches_delivery(&ack(2, true), &delivery));
    assert!(!matches_delivery(&ack(3, false), &delivery));
}

#[test]
fn a_migration_disable_carries_the_approved_set_of_its_revision() {
    use crate::domain::plugin::{ApprovedGrant, DesiredAppState, StateLifecycle};
    let mut state = DesiredAppState {
        app_id: "201".into(),
        binding_id: "b1".into(),
        plugin_id: "example.poll".into(),
        plugin_version: "1.0.0".into(),
        manifest_digest: "sha256:m".into(),
        revision: 3,
        enabled: true,
        configured: true,
        configuration: serde_json::json!({}),
        secrets_from: None,
    }
    .applied_to(None)
    .unwrap();
    state.organization_id = "101".into();
    state.lifecycle = StateLifecycle::Installed;
    state.approved = ApprovedGrant {
        operations: ["poll.vote".to_owned()].into(),
        permissions: ["im:message:read".to_owned()].into(),
        data_scopes: ["conversation".to_owned()].into(),
    };
    let delivery = disabled_delivery(
        &state,
        &deployment(),
        3,
        serde_json::json!({}),
        "sha256:c".into(),
    );
    assert!(!delivery.enabled);
    // The host keeps one state per revision: the same revision must carry the same approved set
    // the convergence task delivered, or the host refuses it as conflicting.
    assert_eq!(delivery.approved_operations, vec!["poll.vote".to_owned()]);
    assert_eq!(delivery.permissions, vec!["im:message:read".to_owned()]);
    assert_eq!(delivery.data_scopes, vec!["conversation".to_owned()]);
}
