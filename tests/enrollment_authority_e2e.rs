//! The half of signed-bundle enrollment that never shipped.
//!
//! A node could always verify and apply a bundle; nothing could issue one.
//! `sign_with_material` was reachable only from test modules, which meant the
//! only thing that had ever produced a valid bundle was a test — and a test
//! proving a test works proves nothing about the product.
//!
//! These drive the shipped `omakure node authority` verbs against real nodes
//! and then hand the result to the real apply path.

mod support;

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

const TOKEN: &str = "authority-e2e-token-with-enough-entropy-000001";
const ORGANIZATION: &str = "authority-e2e-org";
/// The pre-shared bootstrap pair the audience's config commits to by hash.
const BOOTSTRAP_TOKEN: &str = "authority-e2e-bootstrap-token-000000000001";
const BOOTSTRAP_NONCE: &str = "00112233445566778899aabbccddeeff";

fn run_node(workspace: &Path, args: &[String]) -> Output {
    Command::new(support::omakure_bin())
        .arg("--scripts-dir")
        .arg(workspace)
        .arg("--json")
        .arg("node")
        .arg("--node-state-dir")
        .arg(workspace.join(".node-state"))
        .arg("--node-config")
        .arg(workspace.join("node.toml"))
        .args(args)
        .env("OMAKURE_NODE_TEST_MODE", "1")
        .env("OMAKURE_API_TOKEN", TOKEN)
        .output()
        .expect("run node command")
}

fn assert_ok(label: &str, output: &Output) -> Value {
    assert!(
        output.status.success(),
        "node {label} failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = support::json_envelope(&output.stdout);
    assert_eq!(envelope["ok"], true, "node {label}: {envelope}");
    envelope["data"].clone()
}

fn assert_refused(label: &str, output: &Output) -> String {
    assert!(
        !output.status.success(),
        "node {label} was expected to be refused but succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn init(workspace: &Path) -> Value {
    assert_ok("init", &run_node(workspace, &["init".to_string()]));
    assert_ok("status", &run_node(workspace, &["status".to_string()]))
}

/// The shipped constructions, not a plausible-looking re-implementation.
///
/// Domain-separated SHA-256; getting the separator wrong here would make the
/// test fail for a reason that has nothing to do with what it is checking.
fn bootstrap_token_hash(token: &str) -> String {
    omakure::enrollment::hash_bootstrap_token(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn bootstrap_nonce_hash(nonce_hex: &str) -> String {
    let nonce = omakure::enrollment::parse_hex(nonce_hex, 16).expect("nonce is 16 bytes of hex");
    omakure::enrollment::hash_bootstrap_nonce(&nonce)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Configure the audience to accept bundles from this authority.
fn accept_bundles_from(workspace: &Path, authority: &Value) {
    let path = workspace.join("node.toml");
    let config = std::fs::read_to_string(&path).expect("read node config");
    let config = config
        .replace(
            "enrollment = \"disabled\"",
            "enrollment = \"signed-bundle\"",
        )
        .replace(
            "authorities = []",
            &format!(
                "authorities = [{{ key_id = \"{}\", public_key = \"{}\", revoked = false }}]",
                authority["key_id"].as_str().expect("key id"),
                authority["public_key"].as_str().expect("public key")
            ),
        )
        .replace(
            "bootstrap_token_hash = \"\"",
            &format!(
                "bootstrap_token_hash = \"{}\"",
                bootstrap_token_hash(BOOTSTRAP_TOKEN)
            ),
        )
        .replace(
            "bootstrap_nonce_hash = \"\"",
            &format!(
                "bootstrap_nonce_hash = \"{}\"",
                bootstrap_nonce_hash(BOOTSTRAP_NONCE)
            ),
        )
        .replace("id = \"\"", &format!("id = \"{ORGANIZATION}\""));
    std::fs::write(&path, &config).expect("write node config");
    assert!(
        config.contains("enrollment = \"signed-bundle\"") && config.contains("key_id ="),
        "the config edit matched nothing -- `node init` changed its output:\n{config}"
    );
}

/// Give the issuer an organization, so its bundles name one.
fn set_organization(workspace: &Path) {
    let path = workspace.join("node.toml");
    let config = std::fs::read_to_string(&path).expect("read node config");
    std::fs::write(
        &path,
        config.replace("id = \"\"", &format!("id = \"{ORGANIZATION}\"")),
    )
    .expect("write node config");
}

fn issue(issuer: &Path, audience_id: &str) -> Value {
    assert_ok(
        "authority issue",
        &run_node(
            issuer,
            &[
                "authority".to_string(),
                "issue".to_string(),
                "--audience".to_string(),
                audience_id.to_string(),
                "--role".to_string(),
                "conductor".to_string(),
                "--capability".to_string(),
                "remote-run".to_string(),
                "--lifetime-seconds".to_string(),
                "3600".to_string(),
            ],
        ),
    )
}

fn apply(audience: &Path, bundle_hex: &str) -> Output {
    let token_path = audience.join("bootstrap.token");
    std::fs::write(&token_path, BOOTSTRAP_TOKEN).expect("stage the bootstrap token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&token_path).unwrap().permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&token_path, perms).unwrap();
    }
    let bundle_path = audience.join("bundle.hex");
    std::fs::write(&bundle_path, bundle_hex).expect("stage the bundle");
    run_node(
        audience,
        &[
            "enroll".to_string(),
            "apply".to_string(),
            "--bundle-file".to_string(),
            bundle_path.to_string_lossy().into_owned(),
            "--bootstrap-token-file".to_string(),
            token_path.to_string_lossy().into_owned(),
            "--bootstrap-nonce".to_string(),
            BOOTSTRAP_NONCE.to_string(),
        ],
    )
}

/// The whole point: a bundle the product issued is accepted by the product.
#[test]
fn a_bundle_this_fleet_issued_enrolls_a_real_node() {
    let issuer_dir = tempfile::tempdir().expect("issuer workspace");
    let audience_dir = tempfile::tempdir().expect("audience workspace");
    let issuer = issuer_dir.path();
    let audience = audience_dir.path();

    let issuer_status = init(issuer);
    let audience_status = init(audience);
    let issuer_id = issuer_status["identity"]["node_id"].as_str().unwrap();
    let audience_id = audience_status["identity"]["node_id"].as_str().unwrap();

    let authority = assert_ok(
        "authority create",
        &run_node(
            issuer,
            &[
                "authority".to_string(),
                "create".to_string(),
                "--confirmed".to_string(),
            ],
        ),
    );
    assert!(
        !authority.to_string().contains("private"),
        "the authority's private half must never appear in output: {authority}"
    );

    set_organization(issuer);
    accept_bundles_from(audience, &authority);

    let issued = issue(issuer, audience_id);
    assert_eq!(issued["audience_node_id"], audience_id);
    assert_eq!(issued["subject_node_id"], issuer_id);

    let peer = assert_ok(
        "enroll apply",
        &apply(audience, issued["bundle_hex"].as_str().expect("bundle")),
    );
    assert_eq!(
        peer["node_id"], issuer_id,
        "the audience must now trust the issuer: {peer}"
    );
    assert_eq!(peer["state"], "active");
    assert_eq!(peer["role"], "conductor");
}

/// A bundle from an authority the audience does not name must be refused.
///
/// The control that keeps the test above honest: without it, "accepted" could
/// mean the audience accepts anything.
#[test]
fn a_bundle_from_an_unnamed_authority_is_refused() {
    let issuer_dir = tempfile::tempdir().expect("issuer workspace");
    let stranger_dir = tempfile::tempdir().expect("stranger workspace");
    let audience_dir = tempfile::tempdir().expect("audience workspace");
    let issuer = issuer_dir.path();
    let stranger = stranger_dir.path();
    let audience = audience_dir.path();

    init(issuer);
    init(stranger);
    let audience_id = init(audience)["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();

    let trusted = assert_ok(
        "authority create",
        &run_node(
            issuer,
            &[
                "authority".to_string(),
                "create".to_string(),
                "--confirmed".to_string(),
            ],
        ),
    );
    assert_ok(
        "authority create",
        &run_node(
            stranger,
            &[
                "authority".to_string(),
                "create".to_string(),
                "--confirmed".to_string(),
            ],
        ),
    );

    set_organization(stranger);
    // The audience names the *issuer's* authority, and the stranger signs.
    accept_bundles_from(audience, &trusted);

    let issued = issue(stranger, &audience_id);
    let refusal = assert_refused(
        "enroll apply",
        &apply(audience, issued["bundle_hex"].as_str().expect("bundle")),
    );
    assert!(
        refusal.contains("authority"),
        "the refusal should name the authority as the reason: {refusal}"
    );
}

/// Creating an authority twice must refuse rather than rotate the fleet's key.
#[test]
fn an_authority_is_not_replaced_by_running_the_command_again() {
    let dir = tempfile::tempdir().expect("workspace");
    let workspace = dir.path();
    init(workspace);

    let create = |workspace: &Path| {
        run_node(
            workspace,
            &[
                "authority".to_string(),
                "create".to_string(),
                "--confirmed".to_string(),
            ],
        )
    };
    let first = assert_ok("authority create", &create(workspace));
    assert_refused("authority create", &create(workspace));

    let shown = assert_ok(
        "authority show",
        &run_node(workspace, &["authority".to_string(), "show".to_string()]),
    );
    assert_eq!(
        shown, first,
        "the original authority must survive a refused create"
    );
}
