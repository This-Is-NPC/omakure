use super::*;
use pretty_assertions::assert_eq;
use rstest::fixture;
use std::path::PathBuf;

// --- Filesystem-based tests ---

#[fixture]
fn envs_dir() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let envs = tmp.path().join("envs");
    fs::create_dir_all(&envs).unwrap();

    fs::write(envs.join("dev.conf"), "HOST=localhost\nPORT=3000").unwrap();
    fs::write(
        envs.join("prod.conf"),
        "HOST=prod.example.com\nAPI_KEY=secret",
    )
    .unwrap();

    (tmp, envs)
}

#[rstest]
fn test_list_env_files(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    let repo = FsEnvironmentRepository::new(&envs);
    let files = repo.list_env_files().unwrap();

    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["dev.conf", "prod.conf"]);
}

#[rstest]
fn test_list_env_files_skips_active(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    fs::write(envs.join("active"), "dev.conf\n").unwrap();

    let repo = FsEnvironmentRepository::new(&envs);
    let files = repo.list_env_files().unwrap();
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert!(!names.contains(&"active"));
}

#[rstest]
fn test_load_environment_config_no_active(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    let repo = FsEnvironmentRepository::new(&envs);
    let config = repo.load_environment_config().unwrap();

    assert!(config.active.is_none());
}

#[rstest]
fn test_load_environment_config_with_active(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    fs::write(envs.join("active"), "dev.conf\n").unwrap();

    let repo = FsEnvironmentRepository::new(&envs);
    let config = repo.load_environment_config().unwrap();

    assert_eq!(config.active, Some("dev.conf".to_string()));
}

#[test]
#[cfg(unix)]
fn resolve_active_env_ignores_symlink_target() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    let envs = tmp.path().join("envs");
    fs::create_dir_all(&envs).unwrap();
    let outside = tmp.path().join("outside.conf");
    fs::write(&outside, "TOKEN=outside_secret").unwrap();
    symlink(&outside, envs.join("prod.conf")).unwrap();
    fs::write(envs.join("active"), "prod.conf\n").unwrap();

    let resolved = resolve_active_env(&envs);

    assert!(resolved.is_empty());
}

#[rstest]
fn test_set_active_env(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    let repo = FsEnvironmentRepository::new(&envs);

    repo.set_active_env(Some("dev.conf")).unwrap();
    let active = fs::read_to_string(envs.join("active")).unwrap();
    assert_eq!(active.trim(), "dev.conf");
}

#[rstest]
fn test_set_active_env_clear(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    fs::write(envs.join("active"), "dev.conf\n").unwrap();

    let repo = FsEnvironmentRepository::new(&envs);
    repo.set_active_env(None).unwrap();
    assert!(!envs.join("active").exists());
}

#[rstest]
fn test_set_active_env_nonexistent(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    let repo = FsEnvironmentRepository::new(&envs);

    let result = repo.set_active_env(Some("nonexistent.conf"));
    assert!(result.is_err());
}

#[rstest]
fn test_load_environment_config_active_points_to_missing_file(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    fs::write(envs.join("active"), "ghost.conf\n").unwrap();
    let repo = FsEnvironmentRepository::new(&envs);
    let err = repo.load_environment_config().unwrap_err();
    assert!(format!("{}", err).contains("Environment not found"));
}

#[rstest]
fn test_load_active_env_name_skips_comments_only(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    fs::write(envs.join("active"), "# just a comment\n; also comment\n").unwrap();
    let repo = FsEnvironmentRepository::new(&envs);
    let config = repo.load_environment_config().unwrap();
    assert!(config.active.is_none());
}

#[test]
fn test_parse_env_preview_strips_export_prefix() {
    let input = "export GREETING=hello";
    let preview = parse_env_preview(input);
    assert_eq!(preview, vec![("GREETING".to_string(), "hello".to_string())]);
}

#[test]
fn test_parse_env_defaults_strips_export_prefix() {
    let input = "export FOO=bar";
    let parsed = parse_env_defaults(input);
    assert_eq!(parsed.get("foo").map(String::as_str), Some("bar"));
}

#[rstest]
fn test_load_env_preview(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    let repo = FsEnvironmentRepository::new(&envs);
    let preview = repo.load_env_preview(&envs.join("prod.conf")).unwrap();

    assert_eq!(
        preview[0],
        ("HOST".to_string(), "prod.example.com".to_string())
    );
    assert_eq!(preview[1], ("API_KEY".to_string(), "****".to_string()));
}

#[rstest]
fn test_create_show_replace_set_remove_and_delete_env_by_logical_name(
    envs_dir: (TempDir, PathBuf),
) {
    let (_tmp, envs) = envs_dir;
    let repo = FsEnvironmentRepository::new(&envs);

    repo.create_env("qa", &[("HOST", "qa.example.com"), ("API_KEY", "secret")])
        .unwrap();
    assert!(envs.join("qa.conf").is_file());

    assert_eq!(
        repo.load_env_preview_by_name("qa").unwrap(),
        vec![
            ("HOST".to_string(), "qa.example.com".to_string()),
            ("API_KEY".to_string(), "****".to_string()),
        ]
    );

    repo.set_env_param("qa", "PORT", "443").unwrap();
    repo.set_env_param("qa", "HOST", "qa.internal").unwrap();
    assert_eq!(
        fs::read_to_string(envs.join("qa.conf")).unwrap(),
        "HOST=qa.internal\nAPI_KEY=secret\nPORT=443\n"
    );

    repo.remove_env_param("qa", "API_KEY").unwrap();
    assert_eq!(
        fs::read_to_string(envs.join("qa.conf")).unwrap(),
        "HOST=qa.internal\nPORT=443\n"
    );

    repo.replace_env("qa", &[("HOST", "replacement")]).unwrap();
    assert_eq!(
        fs::read_to_string(envs.join("qa.conf")).unwrap(),
        "HOST=replacement\n"
    );

    repo.delete_env("qa").unwrap();
    assert!(!envs.join("qa.conf").exists());
}

#[rstest]
#[case::empty("")]
#[case::suffix("prod.conf")]
#[case::traversal("../prod")]
#[case::slash("team/prod")]
#[case::backslash("team\\prod")]
#[case::leading_dot(".prod")]
#[case::reserved_active("active")]
fn test_env_management_rejects_unsafe_logical_names(
    envs_dir: (TempDir, PathBuf),
    #[case] name: &str,
) {
    let (_tmp, envs) = envs_dir;
    let repo = FsEnvironmentRepository::new(&envs);

    let err = repo.create_env(name, &[("HOST", "example")]).unwrap_err();
    assert!(
        err.to_string().contains("Invalid environment name"),
        "unexpected error for {name:?}: {err}"
    );
}

#[rstest]
fn test_env_management_rejects_symlink_escape(envs_dir: (TempDir, PathBuf)) {
    let (tmp, envs) = envs_dir;
    let outside = tmp.path().join("outside.conf");
    fs::write(&outside, "HOST=outside\n").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, envs.join("escape.conf")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside, envs.join("escape.conf")).unwrap();

    let repo = FsEnvironmentRepository::new(&envs);
    let err = repo.load_env_preview_by_name("escape").unwrap_err();
    assert!(
        err.to_string().contains("Unsafe environment path"),
        "unexpected error: {err}"
    );
}

#[rstest]
fn test_env_management_activate_deactivate_uses_logical_name(envs_dir: (TempDir, PathBuf)) {
    let (_tmp, envs) = envs_dir;
    let repo = FsEnvironmentRepository::new(&envs);

    repo.activate_env("prod").unwrap();
    assert_eq!(
        fs::read_to_string(envs.join("active")).unwrap(),
        "prod.conf\n"
    );

    repo.deactivate_env().unwrap();
    assert!(!envs.join("active").exists());
}
