//! Opt-in three-container acceptance test for unattended signed-bundle enrollment.
//!
//! Run with:
//! `cargo test --test docker_signed_bundle_e2e -- --ignored --nocapture`

use k256::schnorr::SigningKey;
use omakure::enrollment::{self, EnrollmentRole, SignedEnrollmentBundle};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Every Docker call is bounded, so a wedged daemon cannot hang the suite.
///
/// Two different budgets, because two different things are being bounded. An
/// operation on a stack that is already up is fast, and 120s is a generous
/// ceiling for one. A call carrying `--build` may compile this crate inside the
/// container from a cold layer cache, which on an ordinary machine does not fit
/// in two minutes -- and when it did not, `timeout` killed the build and the
/// test reported `compose up failed`, which reads exactly like the product
/// refusing to start. One budget for both made a slow machine indistinguishable
/// from a broken node.
const COMPOSE_OPERATION_TIMEOUT: &str = "120s";
const COMPOSE_BUILD_TIMEOUT: &str = "1800s";

fn bounded_command_within(program: &str, budget: &str) -> Command {
    let mut command = Command::new("timeout");
    command.args(["--foreground", "--kill-after=10s", budget, program]);
    command
}

fn bounded_command(program: &str) -> Command {
    bounded_command_within(program, COMPOSE_OPERATION_TIMEOUT)
}

/// The budget a Compose invocation gets, decided by whether it can build.
fn compose_timeout(args: &[&str]) -> &'static str {
    if args.contains(&"--build") {
        COMPOSE_BUILD_TIMEOUT
    } else {
        COMPOSE_OPERATION_TIMEOUT
    }
}
const COMPOSE_FILE: &str = "ci/compose/compose.signed-bundle.e2e.yaml";

fn compose_project() -> &'static str {
    static PROJECT: OnceLock<String> = OnceLock::new();
    PROJECT
        .get_or_init(|| format!("omakure-signed-bundle-{}", std::process::id()))
        .as_str()
}

struct ComposeGuard {
    root: PathBuf,
    _files: TempDir,
    finalized: bool,
    /// The autojoin target's pre-placed bootstrap pair. The operator provisioned
    /// both machines, so the authority knows them.
    autojoin_token: String,
    autojoin_nonce: [u8; 16],
    autojoin_config: PathBuf,
}

impl ComposeGuard {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let files = TempDir::new().expect("create signed-bundle E2E files");
        let authority_private = [2_u8; 32];
        let authority = SigningKey::from_slice(&authority_private).expect("authority key");
        let authority_id = hex(&[8; 16]);
        let authority_public = hex(authority.verifying_key().to_bytes().as_slice());
        let authority_token = "authority-signed-bundle-token-0123456789".to_string();
        let target_a_token = "target-a-signed-bundle-token-0123456".to_string();
        let target_b_token = "target-b-signed-bundle-token-0123456".to_string();
        let target_a_nonce = [9_u8; 16];
        let target_b_nonce = [10_u8; 16];
        let authority_nonce = [11_u8; 16];

        let authority_management = generate_tokens(&root, files.path(), "signed-authority");
        let target_a_management = generate_tokens(&root, files.path(), "signed-target-a");
        let target_b_management = generate_tokens(&root, files.path(), "signed-target-b");
        let authority_token_path = files.path().join("authority.bootstrap");
        let target_a_token_path = files.path().join("target-a.bootstrap");
        let target_b_token_path = files.path().join("target-b.bootstrap");
        for (path, token) in [
            (&authority_token_path, &authority_token),
            (&target_a_token_path, &target_a_token),
            (&target_b_token_path, &target_b_token),
        ] {
            write_private_token(path, token);
        }

        let authority_config = files.path().join("authority.toml");
        let target_a_config = files.path().join("target-a.toml");
        let target_b_config = files.path().join("target-b.toml");
        for (path, token, nonce) in [
            (&authority_config, &authority_token, authority_nonce),
            (&target_a_config, &target_a_token, target_a_nonce),
            (&target_b_config, &target_b_token, target_b_nonce),
        ] {
            fs::write(
                path,
                signed_config(&authority_id, &authority_public, token, &nonce),
            )
            .expect("write signed-bundle node config");
        }

        let paths = [
            ("OMAKURE_SIGNED_AUTHORITY_CONFIG", &authority_config),
            ("OMAKURE_SIGNED_TARGET_A_CONFIG", &target_a_config),
            ("OMAKURE_SIGNED_TARGET_B_CONFIG", &target_b_config),
            ("OMAKURE_SIGNED_AUTHORITY_TOKEN", &authority_token_path),
            ("OMAKURE_SIGNED_TARGET_A_TOKEN", &target_a_token_path),
            ("OMAKURE_SIGNED_TARGET_B_TOKEN", &target_b_token_path),
        ];
        for (name, path) in paths {
            std::env::set_var(name, path);
        }
        for (name, path) in [
            (
                "OMAKURE_SIGNED_AUTHORITY_TOKENS",
                &authority_management.tokens,
            ),
            ("OMAKURE_SIGNED_AUTHORITY_CURL", &authority_management.curl),
            (
                "OMAKURE_SIGNED_TARGET_A_TOKENS",
                &target_a_management.tokens,
            ),
            ("OMAKURE_SIGNED_TARGET_A_CURL", &target_a_management.curl),
            (
                "OMAKURE_SIGNED_TARGET_B_TOKENS",
                &target_b_management.tokens,
            ),
            ("OMAKURE_SIGNED_TARGET_B_CURL", &target_b_management.curl),
        ] {
            std::env::set_var(name, path);
        }
        // The autojoin target. Its config is written *after* the authority
        // creates its key, because there is no key to name until then.
        let autojoin_token = "autojoin-signed-bundle-token-01234567".to_string();
        let autojoin_nonce = [12_u8; 16];
        let autojoin_management = generate_scoped_tokens(
            &root,
            files.path(),
            "signed-autojoin",
            &["node:read", "enrollment:write"],
        );
        let autojoin_token_path = files.path().join("autojoin.bootstrap");
        write_private_token(&autojoin_token_path, &autojoin_token);
        let autojoin_config = files.path().join("autojoin.toml");
        fs::write(&autojoin_config, []).expect("create autojoin config placeholder");
        std::env::set_var("OMAKURE_SIGNED_AUTOJOIN_CONFIG", &autojoin_config);
        std::env::set_var("OMAKURE_SIGNED_AUTOJOIN_TOKEN", &autojoin_token_path);
        std::env::set_var(
            "OMAKURE_SIGNED_AUTOJOIN_TOKENS",
            &autojoin_management.tokens,
        );
        std::env::set_var("OMAKURE_SIGNED_AUTOJOIN_CURL", &autojoin_management.curl);

        let bundle_paths = [
            ("OMAKURE_SIGNED_AUTHORITY_A_BUNDLE", "authority-a.bundle"),
            ("OMAKURE_SIGNED_TARGET_A_BUNDLE", "target-a.bundle"),
            ("OMAKURE_SIGNED_TARGET_B_BUNDLE", "target-b.bundle"),
        ];
        for (name, file) in bundle_paths {
            std::env::set_var(name, files.path().join(file));
            fs::write(files.path().join(file), []).expect("create bundle placeholder");
        }
        let guard = Self {
            root,
            _files: files,
            finalized: false,
            autojoin_token,
            autojoin_nonce,
            autojoin_config,
        };
        guard.start();
        guard
    }

    fn start(&self) {
        if std::env::var_os("OMAKURE_E2E_INDUCE_PARTIAL_UP").is_some() {
            let partial = compose(&self.root, &["up", "--build", "-d", "signed-authority"]);
            assert!(partial.status.success(), "partial Compose setup failed");
            let failed = compose(
                &self.root,
                &[
                    "up",
                    "--build",
                    "-d",
                    "signed-authority",
                    "missing-induced-failure-service",
                ],
            );
            assert!(
                !failed.status.success(),
                "induced Compose failure unexpectedly passed"
            );
            panic!("induced partial-up failure");
        }
        let output = compose(
            &self.root,
            &[
                "up",
                "--build",
                "-d",
                "signed-authority",
                "signed-target-a",
                "signed-target-b",
            ],
        );
        assert!(
            output.status.success(),
            "signed bundle compose up failed: {}",
            safe_stderr(&output)
        );
    }

    fn finalize(mut self) {
        if let Err(error) = cleanup(&self.root) {
            panic!("signed-bundle Docker cleanup failed: {error}");
        }
        self.finalized = true;
    }
}

impl Drop for ComposeGuard {
    fn drop(&mut self) {
        if !self.finalized {
            if let Err(error) = cleanup(&self.root) {
                eprintln!("signed-bundle Docker cleanup after panic failed: {error}");
            }
        }
    }
}

fn cleanup(root: &Path) -> Result<(), String> {
    let mut failures = Vec::new();
    // `--profile autojoin` is load-bearing: without it `down` leaves the
    // profile's containers, volumes and network behind, and the leak check
    // below reports it as a failure -- which is how this was found.
    let down = compose(
        root,
        &[
            "--profile",
            "autojoin",
            "down",
            "--volumes",
            "--remove-orphans",
        ],
    );
    if !down.status.success() {
        failures.push(format!(
            "compose down status={} stderr={}",
            down.status,
            safe_stderr(&down)
        ));
    }
    for resource in ["container", "network", "volume"] {
        let output = bounded_command("docker")
            .args([
                resource,
                "ls",
                "-q",
                "--filter",
                &format!("label=com.docker.compose.project={}", compose_project()),
            ])
            .output();
        match output {
            Ok(output) if !output.status.success() => failures.push(format!(
                "inspect {resource} status={} stderr={}",
                output.status,
                safe_stderr(&output)
            )),
            Ok(output) if !String::from_utf8_lossy(&output.stdout).trim().is_empty() => {
                failures.push(format!("project-labeled {resource} remains"));
            }
            Ok(_) => {}
            Err(error) => failures.push(format!("inspect {resource}: {error}")),
        }
    }
    cleanup_result(failures)
}

fn cleanup_result(failures: Vec<String>) -> Result<(), String> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::cleanup_result;

    #[test]
    fn cleanup_reports_all_failures() {
        let error = cleanup_result(vec!["down failed".into(), "container remains".into()])
            .expect_err("cleanup failure should be returned");
        assert!(error.contains("down failed"));
        assert!(error.contains("container remains"));
    }
}

#[test]
#[ignore]
fn cleanup_after_induced_partial_up() {
    std::env::set_var("OMAKURE_E2E_INDUCE_PARTIAL_UP", "1");
    let result = std::panic::catch_unwind(ComposeGuard::new);
    std::env::remove_var("OMAKURE_E2E_INDUCE_PARTIAL_UP");
    assert!(result.is_err(), "induced partial-up should fail");
    cleanup(&PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        .expect("induced partial-up cleanup should leave no resources");
}

fn safe_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

fn safe_generation_stderr(output: &Output) -> String {
    let stderr = safe_stderr(output);
    let lower = stderr.to_ascii_lowercase();
    assert!(
        !lower.contains("bearer ") && !lower.contains("$argon2") && !lower.contains("token ="),
        "token generation stderr contained sensitive material"
    );
    stderr
}

fn compose(root: &Path, args: &[&str]) -> Output {
    bounded_command_within("docker", compose_timeout(args))
        .current_dir(root)
        .args(["compose", "-f", COMPOSE_FILE, "-p", compose_project()])
        .args(args)
        .output()
        .expect("run Docker Compose")
}

fn exec(service: &str, args: &[&str]) -> Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    bounded_command("docker")
        .current_dir(root)
        .args([
            "compose",
            "-f",
            COMPOSE_FILE,
            "-p",
            compose_project(),
            "exec",
            "-T",
            service,
        ])
        .args(args)
        .output()
        .expect("run Docker Compose exec")
}

fn write_private_token(path: &Path, token: &str) {
    fs::write(path, token).expect("write bootstrap token");
    let mut permissions = fs::metadata(path)
        .expect("bootstrap token metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    fs::set_permissions(path, permissions).expect("set bootstrap token permissions");
}

fn copy_to_container(service: &str, source: &Path, destination: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = bounded_command("docker")
        .current_dir(root)
        .args([
            "compose",
            "-f",
            COMPOSE_FILE,
            "-p",
            compose_project(),
            "cp",
            source.to_str().expect("UTF-8 test path"),
            &format!("{service}:{destination}"),
        ])
        .output()
        .expect("copy bundle into container");
    assert!(output.status.success(), "copy into container failed");
}

struct ManagementFiles {
    tokens: PathBuf,
    curl: PathBuf,
}

fn generate_tokens(root: &Path, directory: &Path, id: &str) -> ManagementFiles {
    generate_scoped_tokens(root, directory, id, &["node:read"])
}

/// The autojoin target needs `enrollment:write` as well, because the whole
/// exchange is the authority pushing a bundle at its management API. That is
/// the narrowest scope which admits the operation, and it is not enough on its
/// own: the bundle must still verify against the authority named in the
/// target's own config, and the bootstrap token and nonce must match the hashes
/// its operator placed there.
fn generate_scoped_tokens(
    root: &Path,
    directory: &Path,
    id: &str,
    scopes: &[&str],
) -> ManagementFiles {
    let output = bounded_command(env!("CARGO_BIN_EXE_omakure"))
        .current_dir(root)
        .args(["--json", "token", "generate", "--id", id])
        .args(scopes.iter().flat_map(|scope| ["--scope", scope]))
        .output()
        .expect("generate E2E auth token");
    if !output.status.success() {
        panic!(
            "token generation failed: status={} stderr={}",
            output.status,
            safe_generation_stderr(&output)
        );
    }
    let json: Value = serde_json::from_slice(&output.stdout).expect("token JSON");
    let token = json["data"]["token"].as_str().expect("token value");
    let entry = json["data"]["tokens_file_entry"]
        .as_str()
        .expect("token file entry");
    let tokens = directory.join(format!("{id}.tokens.toml"));
    let client = directory.join(format!("{id}.client.token"));
    let curl = directory.join(format!("{id}.curl.conf"));
    fs::write(&tokens, format!("version = 1\n\n{entry}")).expect("write E2E auth token file");
    fs::write(&client, token).expect("write E2E client token file");
    fs::write(
        &curl,
        format!("header = \"Authorization: Bearer {token}\"\n"),
    )
    .expect("write E2E curl config");
    ManagementFiles { tokens, curl }
}

fn signed_config(
    authority_id: &str,
    authority_public: &str,
    token: &str,
    nonce: &[u8; 16],
) -> String {
    format!(
        r#"version = 1

[node]
display_name = "signed-bundle-e2e"

[api]
bind = "127.0.0.1:7878"

[network]
mode = "direct"
relays = []
static_peers = []
direct_bind = "0.0.0.0:7988"
max_message_bytes = 1048576

[trust]
enrollment = "signed-bundle"
allow_remote_cues = false
allow_baseline_push = false
authorities = [{{ key_id = "{authority_id}", public_key = "{authority_public}", revoked = false }}]
bootstrap_token_hash = "{}"
bootstrap_nonce_hash = "{}"

[organization]
id = "omakure"
discovery_secret_ref = ""
"#,
        hex(&enrollment::hash_bootstrap_token(token.as_bytes())),
        hex(&enrollment::hash_bootstrap_nonce(nonce)),
    )
}

fn status(service: &str) -> Value {
    let output = exec(
        service,
        &[
            "curl",
            "--config",
            "/run/secrets/curl.conf",
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "10",
            "http://127.0.0.1:7878/v1/node/status",
        ],
    );
    assert!(
        output.status.success(),
        "node status failed: {}",
        safe_stderr(&output)
    );
    serde_json::from_slice(&output.stdout).expect("node status JSON")
}

fn wait_for_status() {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if ["signed-authority", "signed-target-a", "signed-target-b"]
            .iter()
            .all(|service| {
                let output = exec(service, &["omakure", "--json", "node", "status"]);
                output.status.success()
            })
        {
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
    let ps = compose(&PathBuf::from(env!("CARGO_MANIFEST_DIR")), &["ps"]);
    let logs = compose(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        &["logs", "--no-color", "signed-target-a"],
    );
    panic!(
        "signed-bundle containers did not become ready; ps={} logs={}",
        String::from_utf8_lossy(&ps.stdout),
        String::from_utf8_lossy(&logs.stdout)
    );
}

fn identity(status: &Value) -> (&str, &str) {
    (
        status["data"]["identity"]["node_id"]
            .as_str()
            .expect("node ID"),
        status["data"]["identity"]["public_key"]
            .as_str()
            .expect("public key"),
    )
}

fn copy_certificate(service: &str, destination: &Path) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = bounded_command("docker")
        .current_dir(root)
        .args([
            "compose",
            "-f",
            COMPOSE_FILE,
            "-p",
            compose_project(),
            "cp",
            &format!("{service}:/var/lib/omakure/transport.cert"),
            destination.to_str().expect("UTF-8 certificate path"),
        ])
        .output()
        .expect("copy public transport certificate");
    assert!(output.status.success(), "certificate copy failed");
}

fn container_ip(service: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let id = compose(&root, &["ps", "-q", service]);
    assert!(id.status.success(), "locate Docker service failed");
    let id = String::from_utf8_lossy(&id.stdout).trim().to_string();
    let output = bounded_command("docker")
        .args([
            "inspect",
            "-f",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
            &id,
        ])
        .output()
        .expect("inspect Docker service");
    assert!(output.status.success(), "inspect Docker service failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[allow(clippy::too_many_arguments)]
fn bundle(
    private_key: &[u8; 32],
    bundle_id: [u8; 16],
    organization: &str,
    audience: &str,
    subject: (&str, &str),
    certificate: &[u8],
    issued_at: u64,
    expires_at: u64,
) -> Vec<u8> {
    SignedEnrollmentBundle::sign_with_material(
        private_key,
        bundle_id,
        [8; 16],
        organization.into(),
        audience.into(),
        subject.0.into(),
        enrollment::parse_hex(subject.1, 32)
            .unwrap()
            .try_into()
            .unwrap(),
        certificate[109..141].try_into().unwrap(),
        certificate.try_into().unwrap(),
        EnrollmentRole::Conductor,
        vec!["remote-run".into()],
        issued_at,
        expires_at,
    )
    .unwrap()
    .encode()
}

fn apply(service: &str, bundle_path: &str, token_path: &str, nonce: &[u8; 16]) -> Output {
    exec(
        service,
        &[
            "omakure",
            "--json",
            "node",
            "enroll",
            "apply",
            "--bundle-file",
            bundle_path,
            "--bootstrap-token-file",
            token_path,
            "--bootstrap-nonce",
            &hex(nonce),
        ],
    )
}

#[test]
#[ignore = "requires Docker and runs isolated authority plus two fresh targets"]
fn docker_signed_bundle_enrollment_is_bound_replay_safe_and_restart_stable() {
    let compose_guard = ComposeGuard::new();
    wait_for_status();
    let authority_status = status("signed-authority");
    let target_a_status = status("signed-target-a");
    let target_b_status = status("signed-target-b");
    let (authority_id, authority_key) = identity(&authority_status);
    let (target_a_id, target_a_key) = identity(&target_a_status);
    let (target_b_id, _target_b_key) = identity(&target_b_status);
    let files = compose_guard._files.path();
    let authority_cert_path = files.join("authority.cert");
    let target_a_cert_path = files.join("target-a.cert");
    copy_certificate("signed-authority", &authority_cert_path);
    copy_certificate("signed-target-a", &target_a_cert_path);
    let authority_cert = fs::read(&authority_cert_path).expect("authority certificate");
    let target_a_cert = fs::read(&target_a_cert_path).expect("target A certificate");
    let private_key = [2_u8; 32];
    let now = enrollment::now_seconds();
    let target_a_bundle = bundle(
        &private_key,
        [1; 16],
        "omakure",
        target_a_id,
        (authority_id, authority_key),
        &authority_cert,
        now,
        now + 600,
    );
    let target_b_bundle = bundle(
        &private_key,
        [2; 16],
        "omakure",
        target_b_id,
        (authority_id, authority_key),
        &authority_cert,
        now,
        now + 600,
    );
    let authority_a_bundle = bundle(
        &private_key,
        [3; 16],
        "omakure",
        authority_id,
        (target_a_id, target_a_key),
        &target_a_cert,
        now,
        now + 600,
    );
    let target_b_second_manager_bundle = bundle(
        &private_key,
        [4; 16],
        "omakure",
        target_b_id,
        (target_a_id, target_a_key),
        &target_a_cert,
        now,
        now + 600,
    );
    let wrong_org_bundle = bundle(
        &private_key,
        [5; 16],
        "other-org",
        target_a_id,
        (authority_id, authority_key),
        &authority_cert,
        now,
        now + 600,
    );
    let expired_bundle = bundle(
        &private_key,
        [6; 16],
        "omakure",
        target_a_id,
        (authority_id, authority_key),
        &authority_cert,
        now.saturating_sub(2_000),
        now.saturating_sub(1_000),
    );
    fs::write(files.join("target-a.bundle"), &target_a_bundle).unwrap();
    fs::write(files.join("target-b.bundle"), &target_b_bundle).unwrap();
    fs::write(files.join("authority-a.bundle"), &authority_a_bundle).unwrap();
    fs::write(
        files.join("target-b-second-manager.bundle"),
        &target_b_second_manager_bundle,
    )
    .unwrap();

    let before_target_a = status("signed-target-a")["data"]["trust"].clone();
    for (name, bytes) in [
        ("wrong-org.bundle", wrong_org_bundle),
        ("expired.bundle", expired_bundle),
    ] {
        let path = files.join(name);
        fs::write(&path, bytes).unwrap();
        copy_to_container("signed-target-a", &path, &format!("/tmp/{name}"));
        assert!(!apply(
            "signed-target-a",
            &format!("/tmp/{name}"),
            "/run/secrets/bootstrap-token/bootstrap.token",
            &[9; 16],
        )
        .status
        .success());
        assert_eq!(status("signed-target-a")["data"]["trust"], before_target_a);
    }
    let target_a = apply(
        "signed-target-a",
        "/run/secrets/target-a.bundle",
        "/run/secrets/bootstrap-token/bootstrap.token",
        &[9; 16],
    );
    assert!(
        target_a.status.success(),
        "target A enrollment failed: stdout={} stderr={}",
        String::from_utf8_lossy(&target_a.stdout),
        String::from_utf8_lossy(&target_a.stderr)
    );
    assert!(exec(
        "signed-target-a",
        &[
            "/bin/sh",
            "-c",
            "test ! -e /run/secrets/bootstrap-token/bootstrap.token"
        ],
    )
    .status
    .success());
    assert!(apply(
        "signed-authority",
        "/run/secrets/target-a.bundle",
        "/run/secrets/bootstrap-token/bootstrap.token",
        &[11; 16],
    )
    .status
    .success());

    let cross = files.join("cross.bundle");
    fs::write(&cross, &target_a_bundle).unwrap();
    copy_to_container("signed-target-b", &cross, "/tmp/cross.bundle");
    assert!(exec(
        "signed-target-b",
        &[
            "/bin/sh",
            "-c",
            "chmod 0644 /run/secrets/bootstrap-token/bootstrap.token"
        ],
    )
    .status
    .success());
    assert!(!apply(
        "signed-target-b",
        "/tmp/cross.bundle",
        "/run/secrets/bootstrap-token/bootstrap.token",
        &[10; 16],
    )
    .status
    .success());
    assert_eq!(
        status("signed-target-b")["data"]["trust"]["active_peer_count"],
        0
    );
    assert!(exec(
        "signed-target-b",
        &[
            "/bin/sh",
            "-c",
            "chmod 0600 /run/secrets/bootstrap-token/bootstrap.token"
        ],
    )
    .status
    .success());
    assert!(!apply(
        "signed-target-b",
        "/tmp/cross.bundle",
        "/run/secrets/bootstrap-token/bootstrap.token",
        &[10; 16],
    )
    .status
    .success());

    let second_manager_path = files.join("target-b-second-manager.bundle");
    copy_to_container(
        "signed-target-b",
        &second_manager_path,
        "/tmp/target-b-second-manager.bundle",
    );
    let first = thread::spawn(|| {
        apply(
            "signed-target-b",
            "/run/secrets/target-b.bundle",
            "/run/secrets/bootstrap-token/bootstrap.token",
            &[10; 16],
        )
    });
    let second = thread::spawn(|| {
        apply(
            "signed-target-b",
            "/tmp/target-b-second-manager.bundle",
            "/run/secrets/bootstrap-token/bootstrap.token",
            &[10; 16],
        )
    });
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|output| output.status.success())
            .count(),
        1
    );
    assert_eq!(
        status("signed-target-b")["data"]["trust"]["active_peer_count"],
        1
    );
    assert!(exec(
        "signed-target-b",
        &[
            "/bin/sh",
            "-c",
            "test ! -e /run/secrets/bootstrap-token/bootstrap.token"
        ],
    )
    .status
    .success());

    let authority_endpoint = format!("{}:7988", container_ip("signed-authority"));
    let probe = exec(
        "signed-target-a",
        &[
            "omakure",
            "--json",
            "node",
            "direct-probe",
            "--endpoint",
            &authority_endpoint,
            "--peer-node-id",
            authority_id,
        ],
    );
    assert!(
        probe.status.success(),
        "target probe failed: stdout={} stderr={}",
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr)
    );
    let target_a_endpoint = format!("{}:7988", container_ip("signed-target-a"));
    let reverse_probe = exec(
        "signed-authority",
        &[
            "omakure",
            "--json",
            "node",
            "direct-probe",
            "--endpoint",
            &target_a_endpoint,
            "--peer-node-id",
            target_a_id,
        ],
    );
    assert!(
        reverse_probe.status.success(),
        "authority probe failed: stdout={} stderr={}",
        String::from_utf8_lossy(&reverse_probe.stdout),
        String::from_utf8_lossy(&reverse_probe.stderr)
    );

    let target_a_config = files.join("target-a.toml");
    let revoked_token = "target-a-revoked-authority-token-0123";
    write_private_token(&files.join("target-a-revoked.bootstrap"), revoked_token);
    let revoked_config = signed_config(&hex(&[8; 16]), authority_key, revoked_token, &[9; 16])
        .replace("revoked = false", "revoked = true");
    fs::write(&target_a_config, revoked_config).unwrap();
    std::env::set_var(
        "OMAKURE_SIGNED_TARGET_A_TOKEN",
        files.join("target-a-revoked.bootstrap"),
    );
    let token_refreshed = compose(
        &compose_guard.root,
        &["run", "--rm", "--no-deps", "signed-target-a-token-init"],
    );
    assert!(token_refreshed.status.success(), "refresh token failed");
    let refreshed = compose(
        &compose_guard.root,
        &["run", "--rm", "--no-deps", "signed-target-a-config-init"],
    );
    assert!(
        refreshed.status.success(),
        "refresh revoked authority config failed"
    );
    let restarted = compose(&compose_guard.root, &["restart", "signed-target-a"]);
    assert!(
        restarted.status.success(),
        "restart after authority revocation failed"
    );
    wait_for_status();
    let authority_endpoint = format!("{}:7988", container_ip("signed-authority"));
    let post_restart_probe = exec(
        "signed-target-a",
        &[
            "omakure",
            "--json",
            "node",
            "direct-probe",
            "--endpoint",
            &authority_endpoint,
            "--peer-node-id",
            authority_id,
        ],
    );
    assert!(
        post_restart_probe.status.success(),
        "post-restart target probe failed: stdout={} stderr={}",
        String::from_utf8_lossy(&post_restart_probe.stdout),
        String::from_utf8_lossy(&post_restart_probe.stderr)
    );
    let target_a_endpoint = format!("{}:7988", container_ip("signed-target-a"));
    let post_restart_reverse_probe = exec(
        "signed-authority",
        &[
            "omakure",
            "--json",
            "node",
            "direct-probe",
            "--endpoint",
            &target_a_endpoint,
            "--peer-node-id",
            target_a_id,
        ],
    );
    assert!(
        post_restart_reverse_probe.status.success(),
        "post-restart authority probe failed: stdout={} stderr={}",
        String::from_utf8_lossy(&post_restart_reverse_probe.stdout),
        String::from_utf8_lossy(&post_restart_reverse_probe.stderr)
    );
    let before = status("signed-target-a")["data"]["trust"].clone();
    let revoked_bundle = bundle(
        &private_key,
        [7; 16],
        "omakure",
        target_a_id,
        (authority_id, authority_key),
        &authority_cert,
        now,
        now + 600,
    );
    let revoked_bundle_path = files.join("revoked.bundle");
    fs::write(&revoked_bundle_path, revoked_bundle).unwrap();
    copy_to_container(
        "signed-target-a",
        &revoked_bundle_path,
        "/tmp/revoked.bundle",
    );
    assert!(!apply(
        "signed-target-a",
        "/tmp/revoked.bundle",
        "/run/secrets/bootstrap-token/bootstrap.token",
        &[9; 16],
    )
    .status
    .success());
    assert!(exec(
        "signed-target-a",
        &[
            "/bin/sh",
            "-c",
            "test -e /run/secrets/bootstrap-token/bootstrap.token"
        ],
    )
    .status
    .success());
    assert_eq!(status("signed-target-a")["data"]["trust"], before);
    compose_guard.finalize();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Unattended autojoin
// ---------------------------------------------------------------------------

/// A machine joins its fleet with **no command run on it**.
///
/// Everything the target is given could have been placed by an installer before
/// that machine existed: its public `node.toml`, the authority's public key, and
/// a bootstrap token. It is *not* given a bundle, and could not be — a bundle
/// names the node that will apply it, and that node's identity does not exist
/// until it first starts.
///
/// So the fleet issues one after the fact. The authority, which is the only
/// thing in the product that can now do so, learns the target's identity from
/// its management API, mints a bundle under its own key, and pushes it. The
/// target ends up trusting the authority having had nothing typed into it.
///
/// The assertion that carries the weight is the one about `docker exec`: every
/// command in this test runs against the *authority* container. A test that
/// shelled into the target would prove nothing about unattended.
#[test]
#[ignore = "requires Docker and runs an isolated authority plus a fresh target"]
fn a_provisioned_machine_joins_with_no_command_run_on_it() {
    let guard = ComposeGuard::new();
    wait_for_status();

    // 1. The fleet creates its authority. Nothing could be issued before this.
    let created = exec(
        "signed-authority",
        &[
            "omakure",
            "--json",
            "node",
            "authority",
            "create",
            "--confirmed",
        ],
    );
    assert!(
        created.status.success(),
        "authority create failed: {}",
        safe_stderr(&created)
    );
    let authority: Value = serde_json::from_slice(&created.stdout).expect("authority JSON");
    let key_id = authority["data"]["key_id"].as_str().expect("key id");
    let public_key = authority["data"]["public_key"]
        .as_str()
        .expect("public key");

    // 2. The installer writes the target's public config, naming that authority.
    fs::write(
        &guard.autojoin_config,
        signed_config(
            key_id,
            public_key,
            &guard.autojoin_token,
            &guard.autojoin_nonce,
        ),
    )
    .expect("write the autojoin config");

    // 3. The machine boots. From here on nothing is typed into it.
    let up = compose(
        &guard.root,
        &["--profile", "autojoin", "up", "-d", "signed-autojoin"],
    );
    assert!(
        up.status.success(),
        "autojoin target did not start: {}",
        safe_stderr(&up)
    );

    // 4. The fleet learns its identity the way a manager would -- over the
    //    network, from the authority container. Waiting is done by polling that
    //    same network path, not by sleeping and hoping.
    wait_for_autojoin();
    let status = autojoin_get("/v1/node/status");
    let target_id = status["data"]["identity"]["node_id"]
        .as_str()
        .expect("the fresh node generated an identity on first start")
        .to_string();
    assert!(
        status["data"]["trust"]["active_peer_count"]
            .as_u64()
            .is_some_and(|count| count == 0),
        "the target must start out belonging to nobody: {status}"
    );

    // 5. The fleet issues membership for that identity.
    let issued = exec(
        "signed-authority",
        &[
            "omakure",
            "--json",
            "node",
            "authority",
            "issue",
            "--audience",
            &target_id,
            "--role",
            "conductor",
            "--capability",
            "inventory-health",
            "--capability",
            "notifications",
            "--lifetime-seconds",
            "600",
        ],
    );
    assert!(
        issued.status.success(),
        "authority issue failed: {}",
        safe_stderr(&issued)
    );
    let issued: Value = serde_json::from_slice(&issued.stdout).expect("issued JSON");
    let bundle_hex = issued["data"]["bundle_hex"].as_str().expect("bundle hex");

    // 6. And delivers it.
    let applied = autojoin_post(
        "/v1/node/enrollment/bundle",
        &serde_json::json!({
            "bundle_hex": bundle_hex,
            "bootstrap_nonce": hex(&guard.autojoin_nonce),
        }),
    );
    assert_eq!(
        applied["data"]["state"], "active",
        "the target must have joined: {applied}"
    );

    // 7. Observed from the target's own view, not from the reply we just read.
    let peers = autojoin_get("/v1/node/peers");
    let joined = peers["data"]
        .as_array()
        .expect("peer list")
        .iter()
        .find(|peer| peer["state"] == "active")
        .expect("the target must now have an active peer");
    assert_eq!(
        joined["role"], "conductor",
        "the role the fleet issued must be the role it recorded: {peers}"
    );

    // 8. And it stays joined across restarts.
    //
    // The bootstrap token is consumed and tombstoned on the first success, so a
    // node that treated its absence as a failure would boot exactly once. Twice,
    // because the first restart is the one that finds the token gone and the
    // second is the one that finds the tombstone already reconciled.
    for attempt in 1..=2 {
        let restart = compose(
            &guard.root,
            &["--profile", "autojoin", "restart", "signed-autojoin"],
        );
        assert!(
            restart.status.success(),
            "restart {attempt} failed: {}",
            safe_stderr(&restart)
        );
        wait_for_autojoin();
        let peers = autojoin_get("/v1/node/peers");
        assert!(
            peers["data"]
                .as_array()
                .expect("peer list")
                .iter()
                .any(|peer| peer["state"] == "active"),
            "restart {attempt} lost the membership it had: {peers}"
        );
    }

    // Snapshot before the refusal, so the comparison after it is against a
    // different reading rather than against itself.
    let before_replay = autojoin_get("/v1/node/peers")["data"].clone();

    // 9. A second delivery of the same bundle is refused, and refused *to the
    //    caller* rather than swallowed. Under this model nobody is unwatched:
    //    the fleet that pushed is the fleet that reads the answer.
    let replay = exec(
        "signed-authority",
        &[
            "curl",
            "--config",
            "/run/secrets/autojoin-curl.conf",
            "--silent",
            "--show-error",
            "--max-time",
            "20",
            "--header",
            "Content-Type: application/json",
            "--data",
            &serde_json::json!({
                "bundle_hex": bundle_hex,
                "bootstrap_nonce": hex(&guard.autojoin_nonce),
            })
            .to_string(),
            "http://signed-autojoin:7878/v1/node/enrollment/bundle",
        ],
    );
    let replay: Value = serde_json::from_slice(&replay.stdout).expect("replay JSON");
    assert_eq!(
        replay["ok"], false,
        "replaying a spent bundle must be refused: {replay}"
    );
    assert!(
        replay["error"]["code"]
            .as_str()
            .is_some_and(|code| !code.is_empty()),
        "the refusal must carry a typed code the caller can act on: {replay}"
    );

    // And the refusal changed nothing.
    assert_eq!(
        autojoin_get("/v1/node/peers")["data"],
        before_replay,
        "a refused delivery must leave trust exactly as it was"
    );
}

/// Wait for the autojoin target to answer, from the authority container.
///
/// Deliberately the same path the test then uses. A readiness check that took a
/// different route could go green while the route under test stayed shut.
fn wait_for_autojoin() {
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        let output = exec(
            "signed-authority",
            &[
                "curl",
                "--config",
                "/run/secrets/autojoin-curl.conf",
                "--fail",
                "--silent",
                "--max-time",
                "5",
                "http://signed-autojoin:7878/v1/node/status",
            ],
        );
        if output.status.success() {
            return;
        }
        thread::sleep(Duration::from_millis(500));
    }
    let logs = compose(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        &["logs", "--no-color", "signed-autojoin"],
    );
    panic!(
        "the autojoin target never answered: {}",
        String::from_utf8_lossy(&logs.stdout)
    );
}

/// Read the autojoin target's API **from the authority container**.
///
/// Never `docker exec` against the target. That constraint is the test.
fn autojoin_get(path: &str) -> Value {
    let output = exec(
        "signed-authority",
        &[
            "curl",
            "--config",
            "/run/secrets/autojoin-curl.conf",
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "10",
            &format!("http://signed-autojoin:7878{path}"),
        ],
    );
    assert!(
        output.status.success(),
        "GET {path} on the autojoin target failed: {}",
        safe_stderr(&output)
    );
    serde_json::from_slice(&output.stdout).expect("autojoin JSON")
}

fn autojoin_post(path: &str, body: &Value) -> Value {
    let body = body.to_string();
    let output = exec(
        "signed-authority",
        &[
            "curl",
            "--config",
            "/run/secrets/autojoin-curl.conf",
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "20",
            "--header",
            "Content-Type: application/json",
            "--data",
            &body,
            &format!("http://signed-autojoin:7878{path}"),
        ],
    );
    assert!(
        output.status.success(),
        "POST {path} on the autojoin target failed: {}",
        safe_stderr(&output)
    );
    serde_json::from_slice(&output.stdout).expect("autojoin JSON")
}
