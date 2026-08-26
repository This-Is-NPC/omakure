//! Packaging contract checks for the official node-service container artifacts.
//!
//! These are file-content assertions (not `docker build`). Full image smoke
//! (including fixed uid/gid volume ownership) runs in the Linux CI Docker job.
//! CI does not require a Docker daemon for this test.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

#[test]
fn dockerfile_is_multi_stage_node_entrypoint() {
    let df = read("Dockerfile");
    assert!(
        df.contains("FROM") && df.to_lowercase().contains("as builder"),
        "Dockerfile should be multi-stage with a builder stage"
    );
    assert!(
        df.contains("bash") && df.contains("git") && df.contains("jq"),
        "runtime image must install bash, git, and jq"
    );
    assert!(
        df.contains("0.0.0.0:7878") && df.contains("--allow-non-loopback"),
        "default CMD must bind 0.0.0.0:7878 with --allow-non-loopback"
    );
    assert!(
        df.contains("node") && df.contains("serve"),
        "default command must run the node serve subcommand"
    );
    assert!(
        !df.to_lowercase().contains("python3") || df.contains("deferred") || df.contains("variant"),
        "python/pwsh must not be required in the default image"
    );
}

#[test]
fn dockerignore_excludes_heavy_paths() {
    let ignore = read(".dockerignore");
    for path in ["target", ".git", ".temp"] {
        assert!(
            ignore
                .lines()
                .any(|l| l.trim() == path || l.trim() == format!("{path}/")),
            ".dockerignore should exclude {path}"
        );
    }
}

#[test]
fn compose_example_is_host_loopback_with_workspace_and_tokens_file() {
    let compose = read("compose.yaml");
    assert!(
        compose.contains("127.0.0.1:7878"),
        "compose should publish only on host loopback 127.0.0.1:7878"
    );
    assert!(
        compose.contains("OMAKURE_API_TOKEN"),
        "compose must document legacy OMAKURE_API_TOKEN"
    );
    assert!(
        !compose.lines().any(|line| {
            let line = line.trim_start();
            !line.starts_with('#') && line.starts_with("OMAKURE_API_TOKEN:")
        }),
        "compose must not require legacy OMAKURE_API_TOKEN"
    );
    assert!(
        compose.lines().any(|line| {
            let line = line.trim_start();
            !line.starts_with('#') && line.starts_with("OMAKURE_TOKENS_FILE:")
        }) && compose.contains("/run/secrets/omakure_tokens.toml:ro"),
        "compose must enable and read-only mount the multi-token file"
    );
    assert!(
        compose.contains("/workspace") || compose.contains("workspace"),
        "compose must mount a workspace volume"
    );
    assert!(
        compose.contains("user: \"10001:10001\"") && compose.contains("10001"),
        "compose must keep the fixed image principal"
    );
}

#[test]
fn machine_service_installers_are_explicit_and_preserve_node_state() {
    let shell = read("install.sh");
    for needle in [
        "--install-node-service",
        "--node-tokens-file",
        "--uninstall-node-service",
        "--uninstall-node-state",
        "--confirmed",
        "systemctl enable omakure-node.service",
        "User=omakure",
        "ExecStart=",
        "/var/lib/omakure",
        "/etc/omakure/node.toml",
        "chmod 0640",
        "node serve",
        "com.omakure.node.plist",
        "_omakure",
        "/Library/LaunchDaemons",
        "RunAtLoad",
    ] {
        assert!(
            shell.contains(needle),
            "install.sh should contain {needle:?}"
        );
    }
    assert!(shell.contains("if [[ ! -e \"${config_path}\" ]]"));
    assert!(shell.contains("RESET_NODE_STATE"));
    assert!(shell.contains("validate_native_service_binary_path"));
    assert!(shell.contains("ExecStart=${binary} node serve"));
    assert!(shell.contains("[discovery]\nenabled = false"));
    assert!(
        shell.find("if (( UNINSTALL_NODE_SERVICE ))").unwrap()
            < shell.find("VERSION=\"$(fetch_latest_version").unwrap(),
        "Unix uninstall must exit before release-version resolution"
    );

    let powershell = read("install.ps1");
    for needle in [
        "InstallNodeService",
        "NodeTokensFile",
        "UninstallNodeService",
        "UninstallNodeState",
        "Confirmed",
        "NT AUTHORITY\\LocalService",
        "node serve",
        "ProgramData",
        "sc.exe create OmakureNode",
        "icacls",
        "Set-ExactNodeAcl",
        "Prepare-NodeAclAccess",
        "if (-not (Test-Path $ConfigPath))",
    ] {
        assert!(
            powershell.contains(needle),
            "install.ps1 should contain {needle:?}"
        );
    }
    assert!(!powershell.contains("obj= \"NT SERVICE\\OmakureNode\""));
    assert!(powershell.contains("[discovery]\nenabled = false"));
    let acl_start = powershell.find("function Set-ExactNodeAcl").unwrap();
    let acl_end = powershell[acl_start..]
        .find("function Restore-NodeAcls")
        .map(|offset| acl_start + offset)
        .unwrap();
    assert!(
        !powershell[acl_start..acl_end].contains("BUILTIN\\Administrators"),
        "final PowerShell ACL function must not grant Administrators"
    );
    let installer = read("src/installer.rs");
    assert!(installer.contains("NT AUTHORITY\\\\LocalService"));
    assert!(installer.contains("/setowner"));
    assert!(!installer.contains("obj=\",\n                \"NT SERVICE\\\\OmakureNode\""));
}

#[cfg(unix)]
#[test]
fn unix_uninstall_service_path_skips_release_resolution_and_network() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("commands.log");
    let shim_dir = temp.path().join("bin");
    fs::create_dir(&shim_dir).unwrap();
    for (name, body) in [
        ("uname", "#!/bin/sh\nprintf 'Linux\\n'\n"),
        ("id", "#!/bin/sh\nprintf '0\\n'\n"),
        (
            "systemctl",
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$OMAKURE_TEST_LOG\"\n",
        ),
        ("rm", "#!/bin/sh\nexit 0\n"),
        (
            "curl",
            "#!/bin/sh\nprintf 'network\\n' >> \"$OMAKURE_TEST_LOG\"\nexit 99\n",
        ),
        (
            "wget",
            "#!/bin/sh\nprintf 'network\\n' >> \"$OMAKURE_TEST_LOG\"\nexit 99\n",
        ),
    ] {
        let path = shim_dir.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let output = Command::new("bash")
        .arg(repo_root().join("install.sh"))
        .arg("--uninstall-node-service")
        .env("PATH", format!("{}:/usr/bin:/bin", shim_dir.display()))
        .env("OMAKURE_TEST_LOG", &log)
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let commands = fs::read_to_string(log).unwrap_or_default();
    assert!(commands.contains("disable --now omakure-node.service"));
    assert!(!commands.contains("network"));
}

#[test]
fn hosted_lifecycle_and_docker_certification_are_declared_without_false_results() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("cargo test --test node_service_e2e --test policy_e2e --locked"));
    assert!(ci.contains("docker build --tag omakure-node:ci ."));
    assert!(ci.contains("for path in health ready"));
    assert!(ci.contains("docker volume create"));
    assert!(ci.contains("chown 10001:10001"));
    assert!(ci.contains("chmod 0700 /var/lib/omakure"));
    let release = read(".docs/headless-release.md");
    assert!(release.contains("Hosted Linux, macOS, and Windows CI/release runs remain pending"));
    assert!(release.contains("816 passed"));
}

#[test]
fn deployment_doc_covers_required_topics_and_multi_token() {
    let doc = read(".docs/deployment.md");
    for needle in [
        "API-only",
        "worker",
        "node serve",
        "volume",
        "SQLite",
        "/v1/health",
        "OMAKURE_API_TOKEN",
        "tokens-file",
        "token generate",
        "Argon2id",
        "policy.toml",
        "OMAKURE_POLICY_FILE",
        "legacy_env_token",
        "routes.writes",
    ] {
        assert!(
            doc.contains(needle),
            "deployment.md should mention {needle:?}"
        );
    }
    let lower = doc.to_lowercase();
    assert!(
        lower.contains("multi-token") || lower.contains("tokens-file"),
        "deployment.md must document multi-token / tokens-file auth"
    );
    assert!(
        lower.contains("legacy"),
        "deployment.md must still document legacy token mode"
    );
    assert!(
        lower.contains("load order"),
        "deployment.md must document policy load order"
    );
}

#[test]
fn legacy_engine_command_is_absent_from_current_surfaces() {
    let help = Command::new(env!("CARGO_BIN_EXE_omakure"))
        .arg("--help")
        .output()
        .expect("run omakure --help");
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(!help_text.contains("engine"));

    let help_ai = Command::new(env!("CARGO_BIN_EXE_omakure"))
        .arg("help-ai")
        .output()
        .expect("run omakure help-ai");
    let help_ai_text = String::from_utf8_lossy(&help_ai.stdout);
    assert!(!help_ai_text.contains("omakure engine"));
    assert!(!help_ai_text.contains("\"engine\""));

    for path in [
        "README.md",
        "mise.toml",
        "Dockerfile",
        "compose.yaml",
        ".docs/architecture.md",
        ".docs/requirements.md",
        ".docs/deployment.md",
        ".docs/development.md",
        ".docs/headless-release.md",
        ".docs/headless-migration.md",
        "rebuild-omakure.md",
    ] {
        let text = read(path);
        assert!(!text.contains("omakure engine"), "stale command in {path}");
    }
}

#[test]
fn docs_index_links_deployment() {
    let index = read(".docs/README.md");
    assert!(
        index.contains("deployment.md"),
        ".docs/README.md should link deployment.md"
    );
}

#[test]
fn current_headless_docs_and_tooling_exist_without_obsolete_ui_docs() {
    let root = repo_root();
    for doc in [
        ".docs/README.md",
        ".docs/headless-migration.md",
        ".docs/headless-release.md",
        ".docs/architecture.md",
        ".docs/requirements.md",
        ".docs/development.md",
        ".docs/usage.md",
        ".docs/workspace.md",
        ".docs/scripts-path.md",
        ".docs/release-artifacts.md",
    ] {
        assert!(
            root.join(doc).is_file(),
            "headless documentation is missing {doc}"
        );
    }
    for obsolete in [".docs/tui-screens-and-widgets.md", ".docs/lua-widgets.md"] {
        assert!(
            !root.join(obsolete).exists(),
            "obsolete current surface remains {obsolete}"
        );
    }
    assert!(!read("mise.toml").contains("[tasks.tui]"));
    let readme = read("README.md");
    assert!(readme.contains("omakure doctor"));
    assert!(!readme.contains("omakure --json doctor"));
    assert!(readme.contains("Optional PowerShell"));
    assert!(readme.contains("Optional Python"));
    let mise = read("mise.toml");
    assert!(mise.contains("OMAKURE_API_TOKEN"));
    assert!(mise.contains("openssl rand -hex 32"));
    assert!(mise.contains("--capability all"));
    assert!(mise.contains("cargo run --bin omakure -- node serve"));
    let release = read(".docs/headless-release.md");
    assert!(release.contains("10,520,464 bytes"));
    assert!(release.contains("8,815,352 bytes"));
    assert!(release.contains("-1,705,112 bytes (-16.21%)"));
    assert!(release.contains("27") && release.contains("23") && release.contains("-4"));
    assert!(release.contains("release archive contract is still binary-only"));
    assert!(release.contains("Hosted Linux, macOS, and Windows CI/release runs remain pending"));
}

#[test]
fn headless_source_tree_has_no_tui_theme_or_widget_assets() {
    let root = repo_root();
    for removed in [
        "src/adapters/tui/app.rs",
        "src/adapters/tui/mod.rs",
        "src/adapters/tui/widgets/mod.rs",
        "src/cli/theme.rs",
        "src/lua_widget.rs",
        "src/theme_config.rs",
        "themes/default.toml",
    ] {
        assert!(
            !root.join(removed).exists(),
            "headless package must not retain removed asset {removed}"
        );
    }

    let cargo = read("Cargo.toml").to_lowercase();
    assert!(cargo.contains("name = \"omakure-installer\""));
    assert!(root.join("src/installer.rs").is_file());
    // `mlua` used to be on this list, but it does not belong to this test's
    // contract. This test guards the removal of the TUI *widget* runtime, which
    // is a different Lua from the script kind roadmap item 5 introduced. The
    // widget stays gone; the script runtime is asserted present separately.
    for removed_dependency in ["crossterm", "ratatui", "rattles"] {
        assert!(
            !cargo.contains(removed_dependency),
            "headless package must not declare {removed_dependency}"
        );
    }

    let help = Command::new(env!("CARGO_BIN_EXE_omakure"))
        .arg("--help")
        .output()
        .expect("run omakure --help");
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("theme"));
}

#[test]
fn release_workflows_build_and_package_only_the_headless_binary() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("branches: [test, master]"));
    for platform in ["ubuntu-latest", "macos-latest", "windows-latest"] {
        assert!(ci.contains(platform), "CI matrix must include {platform}");
    }
    assert!(ci.contains("cargo test --all-targets"));
    assert!(ci.contains("cargo build --release --bin omakure"));

    let release = read(".github/workflows/release.yml");
    assert!(release.contains(".github/package-release.sh"));
    assert!(release.contains("cargo test --all-targets"));
    assert!(release.contains("cargo build --release --bin omakure"));
    assert!(release.contains("archive contains only the headless binary"));
    assert!(release.contains("omakure.exe"));
    assert!(!release.contains("themes/") && !release.contains("tui/"));
}

#[cfg(unix)]
#[test]
fn release_tarball_contains_only_the_required_binary() {
    let temp = tempfile::tempdir().expect("create packaging fixture");
    let binary = temp.path().join("omakure");
    fs::copy(env!("CARGO_BIN_EXE_omakure"), &binary).expect("copy binary fixture");
    fs::write(temp.path().join("theme.toml"), "must not be packaged").expect("fixture");
    let archive = temp.path().join("omakure-test.tar.gz");
    let script = repo_root().join(".github/package-release.sh");

    let output = Command::new("bash")
        .arg(script)
        .arg(&binary)
        .arg(&archive)
        .output()
        .expect("run release packager");
    assert!(
        output.status.success(),
        "packager failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let listing = Command::new("tar")
        .args(["-tzf", archive.to_str().expect("archive path")])
        .output()
        .expect("list release archive");
    assert!(listing.status.success());
    assert_eq!(
        String::from_utf8_lossy(&listing.stdout),
        "omakure\n",
        "release archive must contain exactly the root binary"
    );
}

/// The embedded Lua runtime must stay declared and vendored.
///
/// Deliberately separate from the TUI-removal test above. That one guards the
/// `lua_widget` runtime, which is still gone; this one guards the script kind,
/// which is now shipped. Conflating them would let a future edit satisfy one
/// contract by breaking the other.
///
/// `vendored` is the load-bearing half: without it the binary would link
/// against a system Lua and the whole point — a node that needs no Lua
/// installed — would quietly disappear.
#[test]
fn headless_package_declares_the_vendored_lua_script_runtime() {
    let cargo = read("Cargo.toml").to_lowercase();
    assert!(
        cargo.contains("mlua"),
        "the .lua script kind requires the embedded Lua runtime"
    );
    assert!(
        cargo.contains("vendored"),
        "mlua must be vendored, or the binary depends on a system Lua"
    );
    assert!(
        !std::path::Path::new("src/lua_widget.rs").exists(),
        "the TUI Lua widget runtime must stay removed"
    );
}
