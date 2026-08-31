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

#[test]
fn hosted_lifecycle_and_docker_certification_are_declared_without_false_results() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("cargo test --test node_service_e2e --test policy_e2e --locked"));
    assert!(ci.contains("docker build --tag omakure-node:ci ."));
    assert!(ci.contains("for path in health ready"));
    assert!(ci.contains("docker volume create"));
    assert!(ci.contains("chown 10001:10001"));
    assert!(ci.contains("chmod 0700 /var/lib/omakure"));
}

/// What the install automation proves, and what it still does not, has to stay
/// named where a reader looks.
///
/// `docs/installation.md` is that place and is now the only one: the roadmap is
/// no longer tracked and the release-notes document was retired, so a claim
/// corrected here cannot be left standing somewhere else. The installers really
/// do write a systemd unit, a launchd plist, and a Windows service, so the
/// document is held to naming all three as well as the gaps -- a record that
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
        "scripts/tasks/usage-kdl",
        "scripts/tasks/usage-docs",
        "scripts/tasks/operation-catalog",
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
    assert!(mise.contains("OMAKURE_API_TOKEN"));
    assert!(mise.contains("openssl rand -hex 32"));
    assert!(mise.contains("--capability all"));
    assert!(mise.contains("cargo run --bin omakure -- node serve"));
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
    assert!(ci.contains("cargo test --all-targets"));
    assert!(ci.contains("cargo build --release --bin omakure"));

    let release = read(".github/workflows/release.yml");
    assert!(release.contains("scripts/release/package-release.sh"));
    assert!(release.contains("cargo test --all-targets"));
    assert!(release.contains("cargo build --release --bin omakure"));
    assert!(release.contains("archive contains only the headless binary"));
    assert!(release.contains("omakure.exe"));
    assert!(!release.contains("themes/") && !release.contains("tui/"));
    assert!(!ci.contains("macos-14"));
    assert!(!release.contains("macos-14"));
}

#[derive(Debug, PartialEq)]
struct ReleaseMatrixTuple {
    runner: String,
    target: String,
    asset_os: String,
    asset_arch: String,
}

fn release_matrix_tuples(workflow: &str) -> Vec<ReleaseMatrixTuple> {
    let mut tuples = Vec::new();
    let mut current: Option<ReleaseMatrixTuple> = None;

    for line in workflow.lines() {
        if let Some(runner) = line.strip_prefix("          - os: ") {
            if let Some(tuple) = current.take() {
                tuples.push(tuple);
            }
            current = Some(ReleaseMatrixTuple {
                runner: runner.to_string(),
                target: String::new(),
                asset_os: String::new(),
                asset_arch: String::new(),
            });
        } else if let Some(tuple) = current.as_mut() {
            if let Some(target) = line.strip_prefix("            target: ") {
                tuple.target = target.to_string();
            } else if let Some(asset_os) = line.strip_prefix("            asset_os: ") {
                tuple.asset_os = asset_os.to_string();
            } else if let Some(asset_arch) = line.strip_prefix("            asset_arch: ") {
                tuple.asset_arch = asset_arch.to_string();
            }
        }
    }
    if let Some(tuple) = current {
        tuples.push(tuple);
    }
    tuples
}

#[test]
fn release_matrix_associates_every_target_with_its_asset_and_runner() {
    let workflow = read(".github/workflows/release.yml");
    let expected = vec![
        ReleaseMatrixTuple {
            runner: "ubuntu-latest".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            asset_os: "linux".to_string(),
            asset_arch: "x86_64".to_string(),
        },
        ReleaseMatrixTuple {
            runner: "ubuntu-latest".to_string(),
            target: "x86_64-unknown-linux-musl".to_string(),
            asset_os: "linux-musl".to_string(),
            asset_arch: "x86_64".to_string(),
        },
        ReleaseMatrixTuple {
            runner: "ubuntu-24.04-arm".to_string(),
            target: "aarch64-unknown-linux-gnu".to_string(),
            asset_os: "linux".to_string(),
            asset_arch: "aarch64".to_string(),
        },
        ReleaseMatrixTuple {
            runner: "ubuntu-24.04-arm".to_string(),
            target: "aarch64-unknown-linux-musl".to_string(),
            asset_os: "linux-musl".to_string(),
            asset_arch: "aarch64".to_string(),
        },
        ReleaseMatrixTuple {
            runner: "macos-15-intel".to_string(),
            target: "x86_64-apple-darwin".to_string(),
            asset_os: "darwin".to_string(),
            asset_arch: "x86_64".to_string(),
        },
        ReleaseMatrixTuple {
            runner: "macos-15".to_string(),
            target: "aarch64-apple-darwin".to_string(),
            asset_os: "darwin".to_string(),
            asset_arch: "aarch64".to_string(),
        },
        ReleaseMatrixTuple {
            runner: "windows-latest".to_string(),
            target: "x86_64-pc-windows-msvc".to_string(),
            asset_os: "windows".to_string(),
            asset_arch: "x86_64".to_string(),
        },
        ReleaseMatrixTuple {
            runner: "windows-11-arm".to_string(),
            target: "aarch64-pc-windows-msvc".to_string(),
            asset_os: "windows".to_string(),
            asset_arch: "aarch64".to_string(),
        },
    ];

    assert_eq!(release_matrix_tuples(&workflow), expected);
    assert!(workflow.contains("${{ matrix.asset_os }}-${{ matrix.asset_arch }}"));
    assert!(workflow.contains("name: dist-${{ matrix.asset_os }}-${{ matrix.asset_arch }}"));
    assert!(workflow.contains("Run native release binary smoke test"));
    assert!(workflow.contains("Verify musl binary is statically linked"));
    assert!(workflow.contains("musl-gcc -dumpmachine"));
    assert!(workflow.contains("x86-64|x86_64"));
    assert!(workflow.contains("aarch64|ARM"));
    assert!(workflow.contains("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER: musl-gcc"));
    assert!(workflow.contains("windows-11-arm"));
    assert!(
        !workflow.contains("${{ matrix.asset_os }}-x86_64"),
        "release archive names must not hardcode x86_64"
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

#[test]
fn release_tarball_contains_only_the_required_binary() {
    let temp = tempfile::tempdir().expect("create packaging fixture");
    let binary = temp.path().join("omakure");
    fs::copy(env!("CARGO_BIN_EXE_omakure"), &binary).expect("copy binary fixture");
    fs::write(temp.path().join("theme.toml"), "must not be packaged").expect("fixture");
    let archive = temp.path().join("omakure-test.tar.gz");
    let script = repo_root().join("scripts/release/package-release.sh");

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
