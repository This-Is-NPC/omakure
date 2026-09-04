//! Packaging contract checks for the official node-service container artifacts.
//!
//! These are file-content assertions (not `docker build`). Full image smoke
//! (including fixed uid/gid volume ownership) runs in the Linux CI Docker job.
//! CI does not require a Docker daemon for this test.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn read(rel: &str) -> String {
    let contents =
        fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"));
    normalize_line_endings(&contents)
}

/// Path normalized for Git Bash on Windows (MSYS `/c/...` paths).
fn bash_safe_path(path: &Path) -> String {
    #[cfg(windows)]
    {
        let native = path.to_string_lossy().replace('\\', "/");
        let stripped = strip_verbatim_prefix(native);
        bash_safe_drive_path(&stripped)
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}

#[cfg(windows)]
fn strip_verbatim_prefix(mut path: String) -> String {
    let lower = path.to_ascii_lowercase();
    const UNC_PREFIX: &str = "//?/unc/";
    const VERBATIM_PREFIX: &str = "//?/";
    if lower.starts_with(UNC_PREFIX) {
        let rest = path[UNC_PREFIX.len()..].to_string();
        path = format!("//{rest}");
    } else if lower.starts_with(VERBATIM_PREFIX) {
        path = path[VERBATIM_PREFIX.len()..].to_string();
    }
    path
}

#[cfg(windows)]
fn bash_safe_drive_path(path: &str) -> String {
    if let Some((drive, rest)) = path.split_once(':') {
        if drive.len() == 1 && drive.chars().all(|c| c.is_ascii_alphabetic()) {
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            return format!("/{}/{}", drive.to_ascii_lowercase(), rest);
        }
    }
    path.to_string()
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
fn temp_copies_are_ignored_and_docker_excluded() {
    let gitignore = read(".gitignore");
    assert!(
        gitignore.lines().any(|line| line.trim() == ".temp/"),
        ".gitignore must ignore temporary roadmap copies"
    );

    let dockerignore = read(".dockerignore");
    assert!(
        dockerignore
            .lines()
            .any(|line| matches!(line.trim(), ".temp" | ".temp/")),
        ".dockerignore must exclude temporary roadmap copies"
    );
}

#[test]
fn docker_builder_receives_compile_time_fixtures_without_runtime_copy() {
    let df = read("Dockerfile");
    let fixture_copy =
        "COPY fixtures/cli-http-parity.toml fixtures/operation-catalog.toml ./fixtures/";
    let builder_end = df
        .find("FROM builder AS harness-builder")
        .expect("Dockerfile should declare the harness builder after the release builder");
    let builder = &df[..builder_end];
    let fixture_position = builder
        .find(fixture_copy)
        .expect("builder should copy both compile-time fixture manifests");
    let build_position = builder
        .find("RUN cargo build --release --bin omakure")
        .expect("builder should compile the release binary");
    assert!(
        fixture_position < build_position,
        "compile-time fixtures must be available before cargo expands include_str!"
    );

    let runtime = df
        .split_once(" AS runtime")
        .map(|(_, runtime)| runtime)
        .expect("Dockerfile should declare a runtime stage");
    let expected_runtime_copy =
        "COPY --from=builder /src/target/release/omakure /usr/local/bin/omakure";
    let runtime_copies: Vec<_> = runtime
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("COPY ") || line.starts_with("ADD "))
        .collect();
    assert_eq!(
        runtime_copies,
        vec![expected_runtime_copy],
        "runtime stage should copy only the release binary from the builder"
    );
}

#[test]
fn dockerignore_excludes_heavy_paths() {
    let ignore = read(".dockerignore");
    for path in ["target", ".git", ".temp"] {
        assert!(
            ignore
                .lines()
                .any(|line| line.trim() == path || line.trim() == format!("{path}/")),
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

/// The service account's home must not be the node state directory.
///
/// `/var/lib/omakure` is a 0700 secrets directory validated against a closed
/// allow-list: anything in it that the list does not name makes the node refuse
/// to read its own state. An `omakure` command run as this account without
/// `OMAKURE_SCRIPTS_DIR` resolves its default workspace under `$HOME`, so while
/// the state directory was the home directory the product created
/// `/var/lib/omakure/Documents/omakure-scripts` itself -- and from that moment
/// `node status`, `GET /v1/node/status` and `GET /v1/node/health` all returned
/// `registry_invalid: node path has unexpected file type: Documents`.
///
/// It survived a restart, because the directory is still there afterwards, and
/// systemd still reported the unit `active`, so nothing pointed at the cause.
/// Recovery meant knowing to delete a directory the product had made.
#[test]
fn the_service_account_is_not_homed_in_the_node_state_directory() {
    let shell = read("scripts/install/install.sh");
    assert!(
        shell.contains("--home-dir /var/lib/omakure-workspace"),
        "scripts/install/install.sh must home the service account in the workspace"
    );
    assert!(
        !shell.contains("--home-dir /var/lib/omakure "),
        "install.sh homes the service account in the node state directory, so any \
         omakure command run as that account without OMAKURE_SCRIPTS_DIR writes a \
         default workspace into a directory whose closed allow-list then refuses \
         every read of the node's own state"
    );
}

#[test]
fn machine_service_installers_are_explicit_and_preserve_node_state() {
    let shell = read("scripts/install/install.sh");
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
            "scripts/install/install.sh should contain {needle:?}"
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
    assert!(!shell.contains("sync_repo_scripts"));

    let powershell = read("scripts/install/install.ps1");
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
    assert!(!powershell.contains("Copy-RepoScripts"));
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
        .arg(repo_root().join("scripts/install/install.sh"))
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

#[cfg(unix)]
#[test]
fn unix_install_artifact_skips_github_version_lookup() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("commands.log");
    let shim_dir = temp.path().join("shims");
    fs::create_dir(&shim_dir).unwrap();
    for (name, body) in [
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

    let artifact_dir = temp.path().join("artifact-build");
    fs::create_dir_all(&artifact_dir).unwrap();
    let stub = artifact_dir.join("omakure");
    fs::write(
        &stub,
        "#!/bin/sh\ncase \"$1\" in\n  --version|-V|version)\n    echo 'omakure 9.9.9'\n    ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

    let artifact = temp.path().join("omakure-local.tar.gz");
    let tar_status = Command::new("tar")
        .args(["-czf"])
        .arg(&artifact)
        .arg("-C")
        .arg(&artifact_dir)
        .arg("omakure")
        .status()
        .unwrap();
    assert!(
        tar_status.success(),
        "failed to build local artifact tarball"
    );

    let bin_dir = temp.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();

    let output = Command::new("bash")
        .arg(repo_root().join("scripts/install/install.sh"))
        .arg("--artifact")
        .arg(&artifact)
        .arg("--bin-dir")
        .arg(&bin_dir)
        .env("PATH", format!("{}:/usr/bin:/bin", shim_dir.display()))
        .env("OMAKURE_TEST_LOG", &log)
        .env_remove("VERSION")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "install.sh failed: stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("9.9.9"),
        "success line should report the installed binary version, got: {stdout:?}"
    );
    assert!(
        !stdout.contains("v0.2.0"),
        "artifact install must not print a GitHub release tag: {stdout:?}"
    );

    let commands = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !commands.contains("network"),
        "artifact install must not call curl or wget: {commands:?}"
    );
}

#[test]
fn hosted_lifecycle_and_docker_certification_are_declared_without_false_results() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains(
        "run: ./scripts/tasks/check/platform/${{ matrix.platform }} \"${{ matrix.target }}\""
    ));
    let native = read("scripts/tasks/suite/native-tests");
    assert!(
        native.contains("scripts/tasks/atomic/test-lib")
            && native.contains("scripts/tasks/suite/native-integration"),
        "native-tests suite must own native test aggregation"
    );
    assert!(ci.contains("run: ./scripts/tasks/cert/docker-smoke"));
    let docker_smoke = read("scripts/tasks/cert/docker-smoke");
    assert!(docker_smoke.contains("image='omakure-node:ci'"));
    assert!(docker_smoke.contains("docker build --tag \"$image\" \"$root_dir\""));
    assert!(docker_smoke.contains("for path in health ready"));
    assert!(docker_smoke.contains("docker volume create"));
    assert!(docker_smoke.contains("chown 10001:10001"));
    assert!(docker_smoke.contains("chmod 0700 /var/lib/omakure"));
}

/// What the install automation proves, and what it still does not, has to stay
/// named where a reader looks.
///
/// `docs/installation.md` is the authoritative place for this contract. The
/// installers really do write a systemd unit, a launchd plist, and a Windows
/// service, so the document must name all three as well as the gaps -- a record
/// softened into "install automation does not ship" would be false in the other
/// direction.
///
/// Evidence for one platform is exactly when the others quietly get credited
/// too, so what the Fedora machines did *not* establish is pinned as present.
#[test]
fn the_install_and_platform_evidence_is_named_rather_than_implied() {
    // Matched on the sentence rather than on the line wrapping, so reflowing a
    // paragraph is not a failure and deleting the statement is.
    let installation = read("docs/installation.md")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for needle in [
        // What is proven, and by what.
        "No test anywhere runs the install path, registers a real service, or starts one.",
        "the Linux install path has been executed on two real Fedora virtual machines",
        // The limits that survived that evidence.
        "A cold boot into a running node was not separately recorded",
        "`install.ps1` is never executed on any platform, and the launchd path has never been run either.",
        "which is not a service manager starting it at boot",
        // The commands an operator confirms it with.
        "systemctl status omakure-node",
        "launchctl print system/com.omakure.node",
        "sc.exe query OmakureNode",
    ] {
        assert!(
            installation.contains(needle),
            "docs/installation.md must still name {needle:?}"
        );
    }
    assert!(
        !installation.contains("covered by source/static packaging tests and hosted CI"),
        "installation.md must not credit hosted CI with running the installers"
    );
}

#[test]
fn deployment_doc_covers_required_topics_and_multi_token() {
    let doc = read("docs/deployment.md");
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
        "docs/internal/architecture.md",
        "docs/internal/requirements.md",
        "docs/deployment.md",
        "docs/internal/development.md",
        "docs/usage.md",
        "docs/installation.md",
    ] {
        let text = read(path);
        assert!(!text.contains("omakure engine"), "stale command in {path}");
    }
}

#[test]
fn docs_index_links_deployment() {
    let index = read("docs/README.md");
    assert!(
        index.contains("deployment.md"),
        "docs/README.md should link deployment.md"
    );
}

#[test]
fn docs_index_preserves_canonical_manual_ownership() {
    let index = read("docs/README.md");
    for (label, target) in [
        ("CLI and local usage", "usage.md"),
        ("Fleet operations manual", "fleet-operations.md"),
        ("HTTP API", "http-api.md"),
        ("Deployment", "deployment.md"),
        ("AI interface", "ai-interface.md"),
    ] {
        assert!(
            index.contains(&format!("[{label}]({target})")),
            "docs/README.md must link canonical {label} manual"
        );
    }
    let reference = index
        .split_once("## Referência")
        .map(|(_, body)| body)
        .expect("docs/README.md must have Referência section");
    for target in [
        "cli-reference.md",
        "usage/omakure.md",
        "usage/omakure.1",
        "usage/omakure.kdl",
        "operation-catalog.md",
        "operation-support-matrix.md",
        "cli-http-parity.md",
    ] {
        assert!(
            reference.contains(&format!("({target})")),
            "Referência section must link {target}"
        );
    }
}
// These wrappers are Bash scripts with shebangs, so execute them only on Unix,
// where `Command` can launch them directly. The cross-platform Rust freshness
// contracts remain in `cli_reference_contract`: the Clap-rendered reference
// and CLI/HTTP parity checks run on every target.
#[cfg(unix)]
#[test]
fn generated_documentation_checks_are_read_only_and_fresh() {
    let root = repo_root();
    let artifacts = [
        "docs/cli-reference.md",
        "docs/usage/omakure.md",
        "docs/usage/omakure.1",
        "docs/usage/omakure.kdl",
        "docs/usage/fidelity.json",
        "docs/usage/overlay.json",
        "docs/usage/unreportable-semantics.json",
        "docs/usage/fidelity-allowlist.json",
        "docs/operation-catalog.md",
        "docs/operation-support-matrix.md",
    ];
    let before = artifacts
        .iter()
        .map(|path| {
            (
                path,
                fs::read(root.join(path)).expect("read generated artifact"),
            )
        })
        .collect::<Vec<_>>();
    for script in [
        "scripts/tasks/cli-reference",
        "scripts/tasks/atomic/usage-kdl",
        "scripts/tasks/atomic/usage-docs",
        "scripts/tasks/atomic/operation-catalog",
    ] {
        let output = Command::new(root.join(script))
            .arg("--check")
            .current_dir(&root)
            .output()
            .unwrap_or_else(|error| panic!("run {script} --check: {error}"));
        assert!(
            output.status.success(),
            "{script} --check failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for (path, expected) in before {
        assert_eq!(
            fs::read(root.join(path)).expect("re-read generated artifact"),
            expected,
            "{path} changed during a read-only freshness check"
        );
    }
}

#[test]
fn current_headless_docs_and_tooling_exist_without_obsolete_ui_docs() {
    let root = repo_root();
    for doc in [
        "docs/README.md",
        "docs/internal/architecture.md",
        "docs/internal/requirements.md",
        "docs/internal/development.md",
        "docs/usage.md",
        "docs/fleet-operations.md",
        "docs/fleet-model.md",
        "docs/http-api.md",
        "docs/deployment.md",
        "docs/ai-interface.md",
        "docs/cli-reference.md",
        "docs/cli-http-parity.md",
        "docs/operation-catalog.md",
        "docs/operation-support-matrix.md",
        "docs/workspace.md",
        "docs/scripts-path.md",
        "docs/internal/release-artifacts.md",
    ] {
        assert!(
            root.join(doc).is_file(),
            "headless documentation is missing {doc}"
        );
    }
    for obsolete in ["docs/tui-screens-and-widgets.md", "docs/lua-widgets.md"] {
        assert!(
            !root.join(obsolete).exists(),
            "obsolete current surface remains {obsolete}"
        );
    }
    assert!(!read("mise.toml").contains("[tasks.tui]"));
    let readme = read("README.md");
    assert!(readme.contains("cargo run --bin omakure -- doctor"));
    assert!(!readme.contains("omakure --json doctor"));
    assert!(readme.contains("Optional PowerShell or Python"));
    assert!(readme.contains("Lua 5.4 is embedded"));
    let mise = read("mise.toml");
    assert!(
        mise.contains(
            "[tasks.node]\n\
description = \"Run the authenticated machine node service in the foreground\"\n\
run = \"scripts/tasks/atomic/node-serve\"\n\
raw = true"
        ),
        "mise node task must route directly to the canonical atomic"
    );
    let node = read("scripts/tasks/atomic/node-serve");
    assert!(
        node.contains("scripts/tasks/dev/smoke") || node.contains("node serve"),
        "canonical node route must remain a node-service entry point"
    );
    // The archive contract is as-is and keeps its own document.
    let artifacts = read("docs/internal/release-artifacts.md");
    assert!(artifacts.contains("Each archive contains exactly one root entry"));
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
    // `mlua` does not belong to this list. This test guards the removal of the
    // TUI *widget* runtime, which is distinct from the script runtime. The
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
fn automation_scripts_are_canonical_executable_routes() {
    let root = repo_root();
    let mut scripts = Vec::new();
    for directory in [
        "scripts/tasks/atomic",
        "scripts/tasks/suite",
        "scripts/tasks/check/platform",
        "scripts/tasks/cert",
        "scripts/tasks/dev",
        ".githooks",
        "scripts/install",
        "scripts/release",
    ] {
        let entries = fs::read_dir(root.join(directory))
            .unwrap_or_else(|error| panic!("read {directory}: {error}"));
        let mut found = Vec::new();
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_file() {
                found.push(path);
            }
        }
        assert!(
            !found.is_empty(),
            "required automation directory is empty: {directory}"
        );
        scripts.extend(found);
    }
    scripts.extend([
        root.join("scripts/tasks/check/fast"),
        root.join("scripts/tasks/check/full"),
    ]);
    for path in scripts {
        assert!(
            path.is_file(),
            "required automation script is absent: {path:?}"
        );
        #[cfg(unix)]
        if path.extension().and_then(|ext| ext.to_str()) != Some("ps1") {
            assert_ne!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o111,
                0,
                "required POSIX automation script is not executable: {path:?}"
            );
        }
    }
}

#[test]
fn hooks_and_mise_use_one_canonical_script_without_dependencies() {
    let pre_commit = read(".githooks/pre-commit");
    let pre_push = read(".githooks/pre-push");
    assert_eq!(
        pre_commit
            .lines()
            .filter(|line| line.trim() == r#"exec "$root/scripts/tasks/check/fast" "$@""#)
            .count(),
        1,
        "pre-commit must route exactly once to check/fast"
    );
    assert_eq!(
        pre_push
            .lines()
            .filter(|line| line.trim() == r#"exec "$root/scripts/tasks/check/full" "$@""#)
            .count(),
        1,
        "pre-push must route exactly once to check/full"
    );
    assert!(!pre_commit.contains("check/full"));
    assert!(!pre_push.contains("check/fast"));

    let mise = read("mise.toml");
    assert!(
        !mise
            .lines()
            .any(|line| line.trim_start().starts_with("depends")),
        "mise routes must not grow dependency orchestration"
    );
    let root = repo_root();
    assert!(
        !root.join("scripts/mise").exists(),
        "removed scripts/mise directory must stay absent"
    );
    assert!(
        !root.join("scripts/tasks/check/shared").exists(),
        "removed check/shared route must stay absent"
    );
    assert!(
        !mise.contains("scripts/mise/") && !mise.contains("check/shared"),
        "removed script routes must stay absent from Mise"
    );
    let routes = mise
        .lines()
        .filter(|line| line.trim_start().starts_with("run ="))
        .collect::<Vec<_>>();
    assert!(!routes.is_empty(), "mise.toml must declare script routes");
    for line in routes {
        let (_, rest) = line
            .split_once('"')
            .expect("mise run must be a quoted script path");
        let (value, suffix) = rest
            .split_once('"')
            .expect("mise run must have a closing quote");
        assert!(
            suffix.trim().is_empty(),
            "mise run must not append inline commands: {line}"
        );
        assert_eq!(
            value.split_whitespace().count(),
            1,
            "mise run must contain one script path: {value}"
        );
        let path = repo_root().join(value);
        assert!(path.is_file(), "mise route points to no script: {value}");
        #[cfg(unix)]
        assert_ne!(
            fs::metadata(path).unwrap().permissions().mode() & 0o111,
            0,
            "mise route target must be executable: {value}"
        );
    }
}

#[test]
fn native_integration_manifest_matches_every_rust_test_target_once() {
    let script = read("scripts/tasks/suite/native-integration");
    let start = script.match_indices("targets=(").collect::<Vec<_>>();
    assert_eq!(
        start.len(),
        1,
        "native-integration must have one target manifest"
    );
    let body = &script[start[0].0 + "targets=(".len()..];
    let end = body
        .find(')')
        .expect("native-integration manifest must close");
    let manifest = body[..end]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('#').next().unwrap().trim().to_string())
        .collect::<Vec<_>>();
    assert!(
        manifest
            .iter()
            .all(|target| target.split_whitespace().count() == 1),
        "native-integration target manifest entries must be single names"
    );
    let unique = manifest.iter().collect::<BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        manifest.len(),
        "native-integration manifest contains duplicate targets"
    );

    let mut tests = fs::read_dir(repo_root().join("tests"))
        .expect("read tests directory")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("rs"))
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    tests.sort();
    let mut actual = manifest;
    actual.sort();
    assert_eq!(
        actual, tests,
        "native-integration must list every tests/*.rs basename exactly once"
    );
    assert_eq!(
        tests.len(),
        33,
        "the native integration manifest covers 33 tests"
    );
}

#[test]
fn routing_surfaces_do_not_duplicate_tool_or_test_orchestration() {
    let mut surfaces = vec!["scripts/tasks/check/fast", "scripts/tasks/check/full"]
        .into_iter()
        .map(read)
        .collect::<Vec<_>>();
    for directory in ["scripts/tasks/suite", "scripts/tasks/check/platform"] {
        for entry in fs::read_dir(repo_root().join(directory)).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                surfaces.push(read(
                    path.strip_prefix(repo_root()).unwrap().to_str().unwrap(),
                ));
            }
        }
    }
    for surface in surfaces {
        for line in surface.lines() {
            let trimmed = line.trim_start();
            assert!(
                !trimmed.contains("<<"),
                "routing scripts must not embed heredoc test logic"
            );
            for tool in ["cargo ", "docker ", "python ", "python3 ", "openssl "] {
                assert!(
                    !trimmed.contains(tool),
                    "routing surface contains non-atomic {tool:?} logic: {trimmed}"
                );
            }
        }
    }
}

#[test]
fn bounded_and_packaging_atomics_own_shared_interfaces() {
    let bounded = read("scripts/tasks/atomic/run-bounded");
    assert!(
        bounded.contains("duration=\"$1\"")
            && bounded.contains("shift")
            && bounded.contains("exec \"$timeout_bin\"")
            && bounded.contains("exec gtimeout")
            && bounded.contains(
                "atomic/run-bounded: GNU timeout unavailable; operation-level timeout is unavailable and caller/platform CI 60-minute bound applies"
            )
            && bounded.contains("exec \"$@\""),
        "run-bounded must enforce Linux bounds and explicitly report non-Linux fallback"
    );
    let package_suite = read("scripts/tasks/suite/package-release");
    assert!(
        package_suite.contains(r#"exec "$root/scripts/tasks/atomic/package-artifact""#)
            && !package_suite.contains("package-artifact\" \"$@"),
        "package-release suite must package without forwarding arguments to package-artifact"
    );
    assert!(
        package_suite.contains("Linux:x86_64)")
            && package_suite.contains("x86_64-unknown-linux-musl")
            && package_suite.contains(
                r#""$root/scripts/tasks/atomic/build-release" --target-triple "$target""#
            )
            && package_suite.contains(r#""$root/scripts/tasks/atomic/musl-static" "$target""#)
            && package_suite.contains("export OMAKURE_RELEASE_BINARY="),
        "package-release on Linux x86_64 must build musl, verify musl-static, and export OMAKURE_RELEASE_BINARY"
    );
    assert!(
        package_suite.contains(r#""$root/scripts/tasks/atomic/build-release" "$@""#),
        "package-release on other hosts must keep forwarding arguments to build-release"
    );
    let linux_gnu = read("scripts/tasks/check/platform/linux-gnu");
    let local_branch = linux_gnu
        .split_once("if (($# == 0)); then")
        .and_then(|(_, rest)| rest.split_once("else").map(|(branch, _)| branch))
        .expect("linux-gnu must provide a zero-argument local host branch");
    for invocation in [
        "\"$root/scripts/tasks/suite/native-tests\"",
        "\"$root/scripts/tasks/atomic/build-release\"",
        "\"$root/scripts/tasks/atomic/binary-smoke\"",
    ] {
        assert!(
            local_branch.contains(invocation),
            "linux-gnu local branch must invoke {invocation} without target arguments"
        );
    }
    assert!(!local_branch.contains("CARGO_BUILD_TARGET"));
    assert!(!local_branch.contains("--target"));
    assert!(
        linux_gnu
            .contains("\"$root/scripts/tasks/atomic/build-release\" --target-triple \"$target\"")
            && linux_gnu.contains("\"$root/scripts/tasks/atomic/binary-smoke\" \"$target\"")
            && linux_gnu.contains("CARGO_BUILD_TARGET=\"$target\""),
        "linux-gnu explicit target branch must remain target-specific"
    );
    let package_artifact = read("scripts/tasks/atomic/package-artifact");
    assert!(
        package_artifact.contains("scripts/release/package-release.sh")
            && package_artifact.contains("OMAKURE_RELEASE_BINARY:-$root/target/release/omakure")
            && package_artifact.contains("target/release/omakure")
            && package_artifact.contains("\"$root/dist/omakure.tar.gz\"")
            && package_artifact.contains("\"$binary\""),
        "package-artifact must prefer OMAKURE_RELEASE_BINARY and keep the default GNU binary path"
    );
    assert!(
        !package_artifact.contains("rustc -vV")
            && !package_artifact.contains("target/$host")
            && !package_artifact.contains("binary=\"$root/target/$"),
        "package-artifact must not probe alternate target-qualified paths"
    );
}

#[test]
fn ci_and_release_platform_steps_delegate_to_matrix_platform_scripts() {
    for workflow_path in [".github/workflows/ci.yml", ".github/workflows/release.yml"] {
        let workflow = read(workflow_path);
        let step = workflow_step(&workflow, "Run platform checks");
        assert!(
            step.contains("run: ./scripts/tasks/check/platform/${{ matrix.platform }} \"${{ matrix.target }}\""),
            "{workflow_path} platform step must invoke the matrix-selected platform script"
        );
        for line in step.lines() {
            let command = line.trim_start();
            for forbidden in [
                "cargo test",
                "cargo check",
                "cargo build",
                "readelf",
                "binary-smoke",
            ] {
                assert!(
                    !command.starts_with(forbidden),
                    "{workflow_path} platform step must not run {forbidden} directly"
                );
            }
        }
    }
}

fn workflow_step(workflow: &str, name: &str) -> String {
    let marker = format!("      - name: {name}");
    let lines = workflow.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| *line == marker)
        .unwrap_or_else(|| panic!("workflow is missing step {name:?}"));
    lines[start + 1..]
        .iter()
        .take_while(|line| !line.starts_with("      - name: "))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

fn markdown_section(document: &str, heading: &str) -> String {
    let mut section = Vec::new();
    let mut in_section = false;

    for line in document.lines() {
        if line == heading {
            in_section = true;
            continue;
        }
        if in_section && line.starts_with("## ") {
            break;
        }
        if in_section {
            section.push(line);
        }
    }

    section.join("\n")
}

fn workflow_trigger_branches(workflow: &str, event: &str) -> Vec<String> {
    let lines: Vec<&str> = workflow.lines().collect();
    let event_line = format!("  {event}:");
    let event_index = lines
        .iter()
        .position(|line| *line == event_line)
        .unwrap_or_else(|| panic!("workflow is missing the {event} trigger"));
    let branch_index = (event_index + 1..lines.len())
        .take_while(|&index| lines[index].trim().is_empty() || lines[index].starts_with("    "))
        .find(|&index| lines[index].starts_with("    branches:"))
        .unwrap_or_else(|| panic!("{event} trigger is missing branches"));
    let branch_line = lines[branch_index];

    if let Some(inline) = branch_line.strip_prefix("    branches:") {
        let inline = inline.trim();
        if !inline.is_empty() {
            return inline
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|branch| branch.trim().trim_matches(['\'', '"']).to_string())
                .filter(|branch| !branch.is_empty())
                .collect();
        }
    }

    lines[branch_index + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || line.starts_with("      - "))
        .filter_map(|line| line.trim().strip_prefix("- "))
        .map(|branch| branch.trim().trim_matches(['\'', '"']).to_string())
        .collect()
}

fn workflow_job_names(workflow: &str) -> Vec<String> {
    let lines: Vec<&str> = workflow.lines().collect();
    let jobs_index = lines
        .iter()
        .position(|line| *line == "jobs:")
        .expect("workflow is missing jobs");

    lines[jobs_index + 1..]
        .iter()
        .filter(|line| line.starts_with("  ") && !line.starts_with("    "))
        .filter_map(|line| line.trim().strip_suffix(':'))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn workflow_job_field(workflow: &str, job: &str, field: &str) -> String {
    let lines: Vec<&str> = workflow.lines().collect();
    let job_line = format!("  {job}:");
    let job_index = lines
        .iter()
        .position(|line| *line == job_line)
        .unwrap_or_else(|| panic!("workflow is missing the {job} job"));
    let field_prefix = format!("    {field}:");

    lines[job_index + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || line.starts_with("    "))
        .find_map(|line| {
            line.strip_prefix(&field_prefix)
                .map(str::trim)
                .map(str::to_string)
        })
        .unwrap_or_else(|| panic!("{job} job is missing {field}"))
}

#[test]
fn branch_policy_and_release_triggers_are_master_only() {
    let contributing = read("CONTRIBUTING.md");
    let branch_policy = markdown_section(&contributing, "### Default base branch");
    assert!(
        branch_policy.contains("Branch new work off `master`"),
        "contributor flow must branch new work from master"
    );
    assert!(
        branch_policy.contains("Open pull requests directly against `master`"),
        "contributor flow must target master directly"
    );
    assert!(
        !branch_policy.contains("test"),
        "contributor flow must not retain a test-branch policy"
    );

    let ci = read(".github/workflows/ci.yml");
    assert_eq!(
        workflow_trigger_branches(&ci, "push"),
        vec!["master".to_string()]
    );
    assert_eq!(
        workflow_trigger_branches(&ci, "pull_request"),
        vec!["master".to_string()]
    );
    let mut ci_jobs = workflow_job_names(&ci);
    ci_jobs.sort();
    assert_eq!(
        ci_jobs,
        vec![
            "complexity-informational".to_string(),
            "coverage".to_string(),
            "docker-smoke".to_string(),
            "health-plane-certification".to_string(),
            "lint".to_string(),
            "packaging".to_string(),
            "platform".to_string(),
            "release-ready".to_string(),
            "transport-certification".to_string(),
            "usage-artifacts".to_string(),
        ]
    );

    let auto_release = read(".github/workflows/auto-release.yml");
    assert_eq!(
        workflow_trigger_branches(&auto_release, "pull_request_target"),
        vec!["master".to_string()]
    );

    assert_eq!(
        workflow_job_field(&auto_release, "tag", "if"),
        "github.event.pull_request.merged == true"
    );
    assert_eq!(
        workflow_job_field(&auto_release, "release", "uses"),
        "./.github/workflows/release.yml"
    );
}

#[test]
fn release_workflows_build_and_package_only_the_headless_binary() {
    let ci = read(".github/workflows/ci.yml");
    for platform in [
        "ubuntu-latest",
        "ubuntu-24.04-arm",
        "macos-15-intel",
        "macos-15",
        "windows-latest",
        "windows-11-arm",
    ] {
        assert!(ci.contains(platform), "CI matrix must include {platform}");
    }
    assert!(ci.contains("scripts/tasks/check/platform/${{ matrix.platform }}"));

    let release = read(".github/workflows/release.yml");
    assert!(release.contains("scripts/release/package-release.sh"));
    assert!(release.contains("scripts/tasks/check/platform/${{ matrix.platform }}"));
    assert!(release.contains("archive contains only the headless binary"));
    assert!(release.contains("omakure.exe"));
    assert!(!release.contains("themes/") && !release.contains("tui/"));
    assert!(!ci.contains("macos-14"));
    assert!(!release.contains("macos-14"));
    for (platform, runner, target, asset_os, asset_arch) in [
        (
            "linux-gnu",
            "ubuntu-latest",
            "x86_64-unknown-linux-gnu",
            "linux",
            "x86_64",
        ),
        (
            "linux-musl",
            "ubuntu-latest",
            "x86_64-unknown-linux-musl",
            "linux-musl",
            "x86_64",
        ),
        (
            "linux-gnu",
            "ubuntu-24.04-arm",
            "aarch64-unknown-linux-gnu",
            "linux",
            "aarch64",
        ),
        (
            "linux-musl",
            "ubuntu-24.04-arm",
            "aarch64-unknown-linux-musl",
            "linux-musl",
            "aarch64",
        ),
        (
            "macos",
            "macos-15-intel",
            "x86_64-apple-darwin",
            "darwin",
            "x86_64",
        ),
        (
            "macos",
            "macos-15",
            "aarch64-apple-darwin",
            "darwin",
            "aarch64",
        ),
        (
            "windows",
            "windows-latest",
            "x86_64-pc-windows-msvc",
            "windows",
            "x86_64",
        ),
        (
            "windows",
            "windows-11-arm",
            "aarch64-pc-windows-msvc",
            "windows",
            "aarch64",
        ),
    ] {
        let entry = format!(
            "          - platform: {platform}\n            os: {runner}\n            target: {target}\n            asset_os: {asset_os}\n            asset_arch: {asset_arch}"
        );
        assert!(
            release.contains(&entry),
            "release matrix must include exact tuple {platform}/{runner}/{target}"
        );
    }
    let smoke = read("scripts/tasks/atomic/binary-smoke");
    assert!(
        smoke.contains("target/$target/release/omakure")
            && smoke.contains("exec \"$binary\" --version"),
        "binary-smoke must resolve the matrix target and execute --version"
    );
    let musl = read("scripts/tasks/atomic/musl-static");
    assert!(
        musl.contains("target/$target/release/omakure")
            && musl.contains("readelf -l")
            && musl.contains("INTERP"),
        "musl-static must own the static ELF verification"
    );
}

#[cfg(unix)]
/// The installer's preference and the workflow's build must not drift apart.
///
/// If the release stops producing the static archive, `scripts/install/install.sh`
/// silently falls back to the glibc build — and the fallback is deliberately quiet,
/// because it is the normal path for older releases. Nothing would report the
/// loss until an install failed on a machine with an older glibc, which is
/// exactly the machine that cannot be debugged remotely. So the two are
/// pinned to each other here.
#[test]
fn the_installer_preference_and_the_release_matrix_name_the_same_static_asset() {
    let workflow = read(".github/workflows/release.yml");
    assert!(
        workflow.contains("x86_64-unknown-linux-musl"),
        "the release workflow must build the statically linked target"
    );
    assert!(
        workflow.contains("asset_os: linux-musl"),
        "the static build must be published under its own asset name, \
         or it overwrites the glibc archive"
    );
    assert!(
        workflow.contains("musl-tools"),
        "the musl build needs a C toolchain for the vendored native code; \
         without it the job fails at link time"
    );

    let installer = read("scripts/install/install.sh");
    assert!(
        installer.contains("${APP_NAME}-${VERSION}-linux-musl-${arch}.tar.gz"),
        "install.sh must ask for the asset name the workflow publishes"
    );
    assert!(
        installer.contains("download_optional"),
        "the preference must tolerate a release that predates the static \
         asset, rather than failing the install"
    );
    assert!(
        installer.contains("case \"$(uname -m)\""),
        "install.sh must select the asset architecture from the host"
    );

    let powershell = read("scripts/install/install.ps1");
    assert!(
        powershell.contains("PROCESSOR_ARCHITECTURE") && powershell.contains("windows-$arch.zip"),
        "install.ps1 must select the matching Windows architecture asset"
    );
    for architecture in ["PROCESSOR_ARCHITEW6432", "ARM64", "AMD64", "X86"] {
        assert!(
            powershell.contains(architecture),
            "install.ps1 must explicitly handle {architecture}"
        );
    }
    assert!(
        powershell.find("PROCESSOR_ARCHITEW6432").unwrap()
            < powershell.find("PROCESSOR_ARCHITECTURE").unwrap(),
        "install.ps1 must prefer the native architecture under WOW64"
    );
    assert!(
        powershell.contains("Unsupported architecture"),
        "install.ps1 must reject unknown architectures"
    );
}

/// Local `package:release` must archive the same static Linux artifact class as
/// the GitHub `linux-musl` matrix job and `install.sh`.
#[test]
fn local_package_release_on_linux_x86_64_archives_musl_static() {
    let package_suite = read("scripts/tasks/suite/package-release");
    assert!(
        package_suite.contains("x86_64-unknown-linux-musl")
            && package_suite.contains(r#""$root/scripts/tasks/atomic/musl-static" "$target""#)
            && package_suite.contains("export OMAKURE_RELEASE_BINARY="),
        "package-release must build and verify the musl static binary before packaging"
    );

    let package_artifact = read("scripts/tasks/atomic/package-artifact");
    assert!(
        package_artifact.contains("OMAKURE_RELEASE_BINARY:-$root/target/release/omakure"),
        "package-artifact must honor OMAKURE_RELEASE_BINARY when package-release exports it"
    );
}

#[test]
fn release_tarball_contains_only_the_required_binary() {
    let temp = tempfile::tempdir().expect("create packaging fixture");
    let binary = temp.path().join("omakure");
    fs::copy(env!("CARGO_BIN_EXE_omakure"), &binary).expect("copy binary fixture");
    fs::write(temp.path().join("theme.toml"), "must not be packaged").expect("fixture");
    let archive = temp.path().join("omakure-test.tar.gz");
    let script = repo_root().join("scripts/release/package-release.sh");

    let output = Command::new("bash")
        .arg(bash_safe_path(&script))
        .arg(bash_safe_path(&binary))
        .arg(bash_safe_path(&archive))
        .output()
        .expect("run release packager");
    assert!(
        output.status.success(),
        "packager failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let listing = Command::new("tar")
        .args(["-tzf", &bash_safe_path(&archive)])
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
/// which is shipped. Conflating them would let this check pass by breaking the
/// other contract.
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
