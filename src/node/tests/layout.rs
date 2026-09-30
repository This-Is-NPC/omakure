use super::*;

#[test]
fn platform_defaults_match_the_frozen_contract() {
    let linux = default_layout(NodePlatform::Linux, None).unwrap();
    assert_eq!(linux.config_path(), Path::new("/etc/omakure/node.toml"));
    assert_eq!(linux.state_dir(), Path::new("/var/lib/omakure"));

    let mac = default_layout(NodePlatform::MacOs, None).unwrap();
    assert_eq!(
        mac.config_path(),
        Path::new("/Library/Application Support/Omakure/node.toml")
    );
    assert_eq!(
        mac.state_dir(),
        Path::new("/Library/Application Support/Omakure")
    );

    let windows =
        default_layout(NodePlatform::Windows, Some(Path::new("/tmp/ProgramData"))).unwrap();
    assert_eq!(
        windows.config_path(),
        Path::new("/tmp/ProgramData/Omakure/node.toml")
    );
    assert_eq!(windows.state_dir(), Path::new("/tmp/ProgramData/Omakure"));
}

#[cfg(debug_assertions)]
#[test]
fn every_platform_layout_can_initialize_with_shared_defaults() {
    let tmp = tempfile::TempDir::new().unwrap();
    for platform in [
        NodePlatform::Linux,
        NodePlatform::MacOs,
        NodePlatform::Windows,
    ] {
        let root = tmp.path().join(format!("{platform:?}"));
        fs::create_dir(&root).unwrap();
        let (state, config) = if platform == NodePlatform::Linux {
            (root.join("state"), root.join("node.toml"))
        } else {
            let state = root.join("state");
            (state.clone(), state.join("node.toml"))
        };
        let context = NodeContext::resolve_for(
            platform,
            NodePathOverrides::new(Some(state.clone()), Some(config.clone())),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        context.initialize(&NodeConfig::default()).unwrap();
        assert!(state.is_dir());
        assert!(config.is_file());
    }
}

#[cfg(debug_assertions)]
#[test]
fn cli_overrides_env_and_env_overrides_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let cli_state = tmp.path().join("cli-state");
    let cli_config = tmp.path().join("cli.toml");
    let env_state = tmp.path().join("env-state");
    let env_config = tmp.path().join("env.toml");

    let layout = NodeContext::resolve_for(
        NodePlatform::Linux,
        NodePathOverrides::new(Some(cli_state.clone()), Some(cli_config.clone())),
        true,
        Some(env_state.clone()),
        Some(env_config.clone()),
        None,
    )
    .unwrap();
    assert_eq!(layout.state_dir(), cli_state.as_path());
    assert_eq!(layout.config_path(), cli_config.as_path());

    let env_only = NodeContext::resolve_for(
        NodePlatform::Linux,
        NodePathOverrides::default(),
        true,
        Some(env_state.clone()),
        Some(env_config),
        None,
    )
    .unwrap();
    assert_eq!(env_only.state_dir(), env_state.as_path());
    assert!(NodeContext::resolve_for(
        NodePlatform::Linux,
        NodePathOverrides::new(Some(cli_state), Some(cli_config)),
        false,
        None,
        None,
        None,
    )
    .is_err());
}

#[cfg(debug_assertions)]
#[test]
fn constructor_has_no_filesystem_side_effects_and_resolves_identity_paths() {
    let tmp = tempfile::TempDir::new().unwrap();
    let context = test_context(tmp.path());
    assert!(!tmp.path().join("state").exists());
    assert!(!tmp.path().join("node.toml").exists());
    assert_eq!(
        context.identity_path(),
        tmp.path().join("state/identity.key")
    );
    assert_eq!(
        context.database_path(),
        tmp.path().join("state/node.sqlite")
    );
}

#[test]
fn production_rejects_test_environment_overrides() {
    let error = NodeContext::resolve_for(
        NodePlatform::Linux,
        NodePathOverrides::default(),
        false,
        Some(PathBuf::from("/tmp/state")),
        Some(PathBuf::from("/tmp/node.toml")),
        None,
    )
    .unwrap_err();
    assert!(matches!(error, NodeError::TestOverrideOutsideTestMode));
}

#[cfg(debug_assertions)]
#[test]
fn unsafe_paths_are_rejected_without_io() {
    for (state, config) in [
        (PathBuf::from("relative"), PathBuf::from("/tmp/node.toml")),
        (
            PathBuf::from("/tmp/state/../bad"),
            PathBuf::from("/tmp/node.toml"),
        ),
        (PathBuf::from("/tmp/state"), PathBuf::from("relative.toml")),
    ] {
        assert!(NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(Some(state), Some(config)),
            true,
            None,
            None,
            None,
        )
        .is_err());
    }
}

#[cfg(all(windows, debug_assertions))]
#[test]
fn windows_drive_prefix_is_accepted_but_parent_components_are_not() {
    let valid = NodeContext::resolve_for(
        NodePlatform::Windows,
        NodePathOverrides::new(
            Some(PathBuf::from(r"C:\ProgramData\Omakure")),
            Some(PathBuf::from(r"C:\ProgramData\Omakure\node.toml")),
        ),
        true,
        None,
        None,
        None,
    );
    assert!(valid.is_ok());
    let unsafe_path = NodeContext::resolve_for(
        NodePlatform::Windows,
        NodePathOverrides::new(
            Some(PathBuf::from(r"C:\ProgramData\..\Omakure")),
            Some(PathBuf::from(r"C:\ProgramData\Omakure\node.toml")),
        ),
        true,
        None,
        None,
        None,
    );
    assert!(unsafe_path.is_err());
}

#[cfg(all(windows, debug_assertions))]
#[test]
fn windows_native_prefix_is_accepted_for_simulated_unix_layouts() {
    for platform in [NodePlatform::Linux, NodePlatform::MacOs] {
        let result = NodeContext::resolve_for(
            platform,
            NodePathOverrides::new(
                Some(PathBuf::from(r"C:\Temp\Omakure")),
                Some(PathBuf::from(r"C:\Temp\Omakure\node.toml")),
            ),
            true,
            None,
            None,
            None,
        );
        assert!(result.is_ok(), "simulated {platform:?} layout");
    }
}

#[cfg(not(debug_assertions))]
#[test]
fn release_build_rejects_test_mode_even_when_requested() {
    let error = NodeContext::resolve_for(
        NodePlatform::Linux,
        NodePathOverrides::default(),
        true,
        Some(PathBuf::from("/tmp/state")),
        Some(PathBuf::from("/tmp/node.toml")),
        None,
    )
    .unwrap_err();
    assert!(matches!(error, NodeError::TestModeUnavailable));
}
