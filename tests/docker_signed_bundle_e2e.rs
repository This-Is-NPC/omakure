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
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const PROJECT: &str = "omakure-signed-bundle";
const COMPOSE_FILE: &str = "compose.signed-bundle.e2e.yaml";

struct ComposeGuard {
    root: PathBuf,
    _files: TempDir,
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

        let tokens = generate_tokens(&root, files.path());
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
        let bundle_paths = [
            ("OMAKURE_SIGNED_AUTHORITY_A_BUNDLE", "authority-a.bundle"),
            ("OMAKURE_SIGNED_TARGET_A_BUNDLE", "target-a.bundle"),
            ("OMAKURE_SIGNED_TARGET_B_BUNDLE", "target-b.bundle"),
        ];
        for (name, file) in bundle_paths {
            std::env::set_var(name, files.path().join(file));
            fs::write(files.path().join(file), []).expect("create bundle placeholder");
        }
        std::env::set_var("OMAKURE_ENROLLMENT_TOKENS_FILE", tokens);

        compose(&root, &["down", "-v"]);
        let output = compose(
            &root,
            &[
                "up",
                "--build",
                "-d",
                "signed-authority",
                "signed-target-a",
                "signed-target-b",
            ],
        );
        assert!(output.status.success(), "signed bundle compose up failed");
        Self {
            root,
            _files: files,
        }
    }
}

impl Drop for ComposeGuard {
    fn drop(&mut self) {
        let _ = compose(&self.root, &["down", "-v"]);
    }
}

fn compose(root: &Path, args: &[&str]) -> Output {
    Command::new("docker")
        .current_dir(root)
        .args(["compose", "-f", COMPOSE_FILE, "-p", PROJECT])
        .args(args)
        .output()
        .expect("run Docker Compose")
}

fn exec(service: &str, args: &[&str]) -> Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Command::new("docker")
        .current_dir(root)
        .args([
            "compose",
            "-f",
            COMPOSE_FILE,
            "-p",
            PROJECT,
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
    let output = Command::new("docker")
        .current_dir(root)
        .args([
            "compose",
            "-f",
            COMPOSE_FILE,
            "-p",
            PROJECT,
            "cp",
            source.to_str().expect("UTF-8 test path"),
            &format!("{service}:{destination}"),
        ])
        .output()
        .expect("copy bundle into container");
    assert!(output.status.success(), "copy into container failed");
}

fn generate_tokens(root: &Path, directory: &Path) -> PathBuf {
    let output = Command::new(env!("CARGO_BIN_EXE_omakure"))
        .current_dir(root)
        .args([
            "--json",
            "token",
            "generate",
            "--id",
            "signed-bundle-e2e",
            "--scope",
            "node:read",
            "--scope",
            "enrollment:read",
            "--scope",
            "enrollment:write",
        ])
        .output()
        .expect("generate E2E auth token");
    assert!(output.status.success(), "auth token generation failed");
    let json: Value = serde_json::from_slice(&output.stdout).expect("token JSON");
    let entry = json["data"]["tokens_file_entry"]
        .as_str()
        .expect("token file entry");
    let path = directory.join("tokens.toml");
    fs::write(&path, format!("version = 1\n\n{entry}")).expect("write E2E auth token file");
    path
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
    let output = exec(service, &["omakure", "--json", "node", "status"]);
    assert!(output.status.success(), "node status failed");
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
    let output = Command::new("docker")
        .current_dir(root)
        .args([
            "compose",
            "-f",
            COMPOSE_FILE,
            "-p",
            PROJECT,
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
    let output = Command::new("docker")
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
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
