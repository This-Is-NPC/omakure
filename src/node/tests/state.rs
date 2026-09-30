use super::*;

#[cfg(debug_assertions)]
#[test]
fn initialization_is_explicit_atomic_and_does_not_create_identity_or_database() {
    let tmp = tempfile::TempDir::new().unwrap();
    let context = test_context(tmp.path());
    let result = context.initialize(&NodeConfig::default()).unwrap();
    assert!(result.state_dir_created);
    assert!(result.config_created);
    assert!(context.state_dir().is_dir());
    assert!(context.config_path().is_file());
    assert!(!context.identity_path().exists());
    assert!(!context.database_path().exists());
    assert!(NodeConfig::parse(&fs::read_to_string(context.config_path()).unwrap()).is_ok());
    let second = context.initialize(&NodeConfig::default()).unwrap();
    assert!(!second.state_dir_created);
    assert!(!second.config_created);
}

#[cfg(debug_assertions)]
#[test]
fn initialization_rejects_missing_parents_and_symlink_boundaries() {
    let tmp = tempfile::TempDir::new().unwrap();
    let missing = NodeContext::resolve_for(
        NodePlatform::Linux,
        NodePathOverrides::new(
            Some(tmp.path().join("missing/state")),
            Some(tmp.path().join("missing/node.toml")),
        ),
        true,
        None,
        None,
        None,
    )
    .unwrap();
    assert!(matches!(
        missing.initialize(&NodeConfig::default()),
        Err(NodeError::UnsafePath(_))
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = tempfile::TempDir::new().unwrap();
        let link = tmp.path().join("link");
        symlink(outside.path(), &link).unwrap();
        let context = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(Some(link.join("state")), Some(tmp.path().join("node.toml"))),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(matches!(
            context.initialize(&NodeConfig::default()),
            Err(NodeError::UnsafePath(_))
        ));
    }
}

#[cfg(debug_assertions)]
#[test]
fn failed_config_initialization_cleans_up_a_new_state_directory() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state = tmp.path().join("state");
    let config = tmp.path().join("node.toml");
    fs::write(&config, "version = 2\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config, fs::Permissions::from_mode(0o640)).unwrap();
    }
    let context = NodeContext::resolve_for(
        NodePlatform::Linux,
        NodePathOverrides::new(Some(state.clone()), Some(config)),
        true,
        None,
        None,
        None,
    )
    .unwrap();
    assert!(context.initialize(&NodeConfig::default()).is_err());
    assert!(!state.exists());
}
