//! Packaging contract checks for the official engine container artifacts.
//!
//! These are file-content assertions (not `docker build`). Full image smoke
//! (including volume ownership / `--user "$(id -u):$(id -g)"`) lives in
//! `.docs/deployment.md`. CI does not require a Docker daemon for this test.

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
fn dockerfile_is_multi_stage_engine_entrypoint() {
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
        df.contains("engine"),
        "default command must run the engine subcommand"
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
        compose.contains("user:")
            || compose.to_lowercase().contains("non-root")
            || compose.contains("1000"),
        "compose should guide non-root operation"
    );
}

#[test]
fn deployment_doc_covers_required_topics_and_multi_token() {
    let doc = read(".docs/deployment.md");
    for needle in [
        "API-only",
        "worker",
        "engine",
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
    assert!(mise.contains("cargo run --bin omakure -- engine"));
    let release = read(".docs/headless-release.md");
    assert!(release.contains("10,520,464 bytes"));
    assert!(release.contains("8,815,352 bytes"));
    assert!(release.contains("-1,705,112 bytes (-16.21%)"));
    assert!(release.contains("27") && release.contains("23") && release.contains("-4"));
    assert!(release.contains("3,379,669"));
    assert!(release.contains("contained only the root `omakure` binary"));
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
    for removed_dependency in ["crossterm", "ratatui", "rattles", "mlua"] {
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
