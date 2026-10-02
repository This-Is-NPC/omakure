use super::*;

/// Deny-all is right for every unreadable config. Silence about *which*
/// failure it was is not.
///
/// A mode bit that turns remote Cues off must not be indistinguishable from
/// a node that simply never opted in — those are the same decision and
/// completely different operator problems, and the reason is the only thing
/// that tells them apart.
#[cfg(all(unix, debug_assertions))]
#[test]
fn an_unreadable_policy_config_is_distinguishable_from_nothing_declared() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let context = test_context(temp.path());
    let path = temp.path().join("node.toml");
    let valid = NodeConfig::default().to_toml().unwrap();

    // No config at all: nothing was declared.
    assert!(matches!(
        read_policy_config(&context),
        PolicyConfig::NothingDeclared
    ));

    // The passing control. Without it the assertions below would hold just
    // as well if the reader never returned anything but a failure.
    fs::write(&path, &valid).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(matches!(
        read_policy_config(&context),
        PolicyConfig::Declared(_)
    ));

    // A mode the node refuses: unreadable, and the reason names the file
    // and the mode so the operator can act on it.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let PolicyConfig::Unreadable(mode_reason) = read_policy_config(&context) else {
        panic!("a config this node refuses to read must not look like nothing declared");
    };
    assert!(
        mode_reason.contains(&path.display().to_string()) && mode_reason.contains("0644"),
        "the reason must name the file and the mode: {mode_reason}"
    );

    // A different failure must read differently, or the reason carries no
    // information beyond "something went wrong".
    fs::write(&path, "this is not toml = = =").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let PolicyConfig::Unreadable(parse_reason) = read_policy_config(&context) else {
        panic!("a config that will not parse must not look like nothing declared");
    };
    assert!(
        parse_reason.contains(&path.display().to_string()),
        "the reason must name the file: {parse_reason}"
    );
    assert_ne!(
        mode_reason, parse_reason,
        "a permissions failure and a malformed config must not read the same"
    );
}
