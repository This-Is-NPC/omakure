use super::*;

#[test]
fn unresolved_discovery_secret_keeps_stable_operation_error() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let mut config = NodeConfig::default();
    config.discovery.enabled = true;
    config.organization.discovery_secret_ref = "secret://prod/absent".into();
    initialize_node(&context, &config).unwrap();

    let error = scan_discovery(&context, temp.path(), 0, false).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::DiscoverySecretMismatch);
    assert_eq!(error.message, "discovery secret could not be resolved");
    assert!(!error.message.contains("secret://prod/absent"));
}
