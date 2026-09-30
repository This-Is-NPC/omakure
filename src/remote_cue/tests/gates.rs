use super::*;
use crate::node_registry::health::HealthAuthorization;
use crate::node_registry::{PeerRole, PeerState};

fn authorization(role: PeerRole, state: PeerState, capabilities: &[&str]) -> HealthAuthorization {
    HealthAuthorization {
        node_id: "omk1_test".to_string(),
        state,
        role,
        capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
    }
}

fn passing() -> LocalAuthority {
    LocalAuthority {
        remote_cues_enabled: true,
        declared_scripts: vec!["deploy.sh".to_string()],
        declared_batteries: Vec::new(),
        authorization: Some(authorization(
            PeerRole::Conductor,
            PeerState::Active,
            &[CAPABILITY_REMOTE_RUN, CAPABILITY_NOTIFICATIONS],
        )),
    }
}

#[test]
fn all_four_gates_passing_accepts() {
    assert_eq!(evaluate_gates(&passing()), GateDecision::Accepted);
}

/// Each gate flipped alone, with the other three passing.
///
/// This is the shape of the certification: a gate that only refuses when
/// several things are wrong at once is not a gate.
#[test]
fn gate_a_alone_refuses_when_the_node_has_not_opted_in() {
    let mut authority = passing();
    authority.remote_cues_enabled = false;
    assert_eq!(
        evaluate_gates(&authority),
        GateDecision::Rejected(CueCode::Disabled)
    );
}

#[test]
fn gate_b_alone_refuses_an_unknown_peer() {
    let mut authority = passing();
    authority.authorization = None;
    assert_eq!(
        evaluate_gates(&authority),
        GateDecision::Rejected(CueCode::NotActiveConductor)
    );
}

#[test]
fn gate_b_alone_refuses_a_performer() {
    let mut authority = passing();
    authority.authorization = Some(authorization(
        PeerRole::Performer,
        PeerState::Active,
        &[CAPABILITY_REMOTE_RUN, CAPABILITY_NOTIFICATIONS],
    ));
    assert_eq!(
        evaluate_gates(&authority),
        GateDecision::Rejected(CueCode::NotActiveConductor)
    );
}

#[test]
fn gate_b_alone_refuses_a_revoked_conductor() {
    let mut authority = passing();
    authority.authorization = Some(authorization(
        PeerRole::Conductor,
        PeerState::Revoked,
        &[CAPABILITY_REMOTE_RUN, CAPABILITY_NOTIFICATIONS],
    ));
    assert_eq!(
        evaluate_gates(&authority),
        GateDecision::Rejected(CueCode::NotActiveConductor)
    );
}

#[test]
fn gate_c_alone_refuses_a_conductor_without_remote_run() {
    let mut authority = passing();
    authority.authorization = Some(authorization(
        PeerRole::Conductor,
        PeerState::Active,
        &[CAPABILITY_NOTIFICATIONS],
    ));
    assert_eq!(
        evaluate_gates(&authority),
        GateDecision::Rejected(CueCode::MissingRemoteRun)
    );
}

#[test]
fn gate_d_alone_refuses_a_conductor_that_could_not_receive_the_outcome() {
    let mut authority = passing();
    authority.authorization = Some(authorization(
        PeerRole::Conductor,
        PeerState::Active,
        &[CAPABILITY_REMOTE_RUN],
    ));
    assert_eq!(
        evaluate_gates(&authority),
        GateDecision::Rejected(CueCode::MissingNotifications)
    );
}

/// Gate A is evaluated before anything peer-specific, so a node that has
/// not opted in cannot be probed for what it knows about a peer.
#[test]
fn a_disabled_node_refuses_identically_for_every_peer() {
    let mut unknown = passing();
    unknown.remote_cues_enabled = false;
    unknown.authorization = None;

    let mut known = passing();
    known.remote_cues_enabled = false;

    assert_eq!(evaluate_gates(&unknown), evaluate_gates(&known));
}

#[test]
fn trust_role_and_capability_refusals_are_never_reported_to_the_sender() {
    for code in [
        CueCode::Disabled,
        CueCode::NotActiveConductor,
        CueCode::MissingRemoteRun,
        CueCode::MissingNotifications,
    ] {
        assert!(
            !code.is_reportable(),
            "{} must be audited silently",
            code.name()
        );
    }
    for code in [
        CueCode::NotDeclared,
        CueCode::ScriptUnresolvable,
        CueCode::Expired,
        CueCode::Duplicate,
        CueCode::RateLimited,
        CueCode::RunAlreadyInFlight,
        CueCode::InvalidMessage,
        CueCode::ScriptDeclaresSecrets,
    ] {
        assert!(code.is_reportable(), "{} may be answered", code.name());
    }
}

/// Nothing runs remotely unless someone wrote it down.
#[test]
fn an_undeclared_script_is_refused_even_with_every_trust_gate_passing() {
    assert_eq!(
        is_declared("deploy.sh", &["restart.lua".to_string()]),
        Err(CueCode::NotDeclared)
    );
}

/// The switch that matters most: enabling remote Cues grants nothing on its
/// own. Two independent deliberate acts are required.
#[test]
fn an_empty_declaration_denies_everything() {
    assert_eq!(is_declared("deploy.sh", &[]), Err(CueCode::NotDeclared));
    assert_eq!(is_declared("", &[]), Err(CueCode::NotDeclared));
}

#[test]
fn a_declared_script_passes_the_fifth_gate() {
    let declared = vec!["deploy.sh".to_string(), "restart.lua".to_string()];
    assert_eq!(is_declared("deploy.sh", &declared), Ok(()));
    assert_eq!(is_declared("restart.lua", &declared), Ok(()));
}

/// Declaration is an exact match, so a near-miss cannot slip through.
#[test]
fn declaration_is_not_a_prefix_or_suffix_match() {
    let declared = vec!["deploy.sh".to_string()];
    for near in [
        "deploy",
        "deploy.sh.bak",
        "Deploy.sh",
        "xdeploy.sh",
        "deploy.sh ",
    ] {
        assert_eq!(
            is_declared(near, &declared),
            Err(CueCode::NotDeclared),
            "{near:?} must not match a declaration of deploy.sh"
        );
    }
}

/// A node that cannot read its own config has declared nothing.
#[test]
fn the_default_policy_denies_everything() {
    let policy = CuePolicy::default();
    assert!(!policy.enabled);
    assert!(policy.declared_scripts.is_empty());
    assert_eq!(
        is_declared("deploy.sh", &policy.declared_scripts),
        Err(CueCode::NotDeclared)
    );
}

/// Audited distinctly, reported indistinguishably.
fn workspace_with_battery_script(
    battery: &str,
    script_name: &str,
) -> (
    tempfile::TempDir,
    crate::workspace::Workspace,
    std::path::PathBuf,
) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = crate::workspace::Workspace::new(dir.path().to_path_buf());
    let installed = dir.path().join(script_name);
    std::fs::write(&installed, "#!/usr/bin/env bash\n").unwrap();

    let record_dir = workspace
        .omakure_dir()
        .join("batteries")
        .join("installed")
        .join(battery);
    std::fs::create_dir_all(&record_dir).unwrap();
    std::fs::write(
        record_dir.join("record.json"),
        serde_json::json!({
            "battery_name": battery,
            "script_id": format!("{battery}.{script_name}"),
            "git_url": "https://example.invalid/b.git",
            "requested_ref": "main",
            "resolved_commit": "0".repeat(40),
            "source_path": script_name,
            "installed_path": installed,
        })
        .to_string(),
    )
    .unwrap();
    (dir, workspace, installed)
}

fn policy(scripts: &[&str], batteries: &[&str]) -> CuePolicy {
    CuePolicy {
        enabled: true,
        declared_scripts: scripts.iter().map(|s| s.to_string()).collect(),
        declared_batteries: batteries.iter().map(|b| b.to_string()).collect(),
    }
}

/// Declaring a battery declares its installed scripts.
#[test]
fn a_script_from_a_declared_battery_passes_without_being_named() {
    let (_dir, workspace, installed) = workspace_with_battery_script("azure", "rg-list.sh");
    assert_eq!(
        is_declared_or_from_declared_battery(
            "rg-list.sh",
            &installed,
            &policy(&[], &["azure"]),
            &workspace
        ),
        Ok(())
    );
}

/// And declaring one battery does not declare another.
#[test]
fn a_script_from_an_undeclared_battery_is_refused() {
    let (_dir, workspace, installed) = workspace_with_battery_script("azure", "rg-list.sh");
    assert_eq!(
        is_declared_or_from_declared_battery(
            "rg-list.sh",
            &installed,
            &policy(&[], &["aws"]),
            &workspace
        ),
        Err(CueCode::NotDeclared)
    );
}

/// A hand-written script that no battery installed stays undeclared, even
/// when it sits beside battery scripts in the same workspace.
#[test]
fn a_script_with_no_provenance_is_refused_despite_a_declared_battery() {
    let (dir, workspace, _installed) = workspace_with_battery_script("azure", "rg-list.sh");
    let local = dir.path().join("local.sh");
    std::fs::write(&local, "#!/usr/bin/env bash\n").unwrap();
    assert_eq!(
        is_declared_or_from_declared_battery(
            "local.sh",
            &local,
            &policy(&[], &["azure"]),
            &workspace
        ),
        Err(CueCode::NotDeclared)
    );
}

/// Naming a script still works, with or without batteries in play.
#[test]
fn an_explicitly_named_script_needs_no_battery() {
    let (dir, workspace, _installed) = workspace_with_battery_script("azure", "rg-list.sh");
    let local = dir.path().join("deploy.sh");
    std::fs::write(&local, "#!/usr/bin/env bash\n").unwrap();
    assert_eq!(
        is_declared_or_from_declared_battery(
            "deploy.sh",
            &local,
            &policy(&["deploy.sh"], &[]),
            &workspace
        ),
        Ok(())
    );
}

/// Declaring nothing still denies everything.
#[test]
fn an_empty_policy_denies_even_battery_scripts() {
    let (_dir, workspace, installed) = workspace_with_battery_script("azure", "rg-list.sh");
    assert_eq!(
        is_declared_or_from_declared_battery(
            "rg-list.sh",
            &installed,
            &policy(&[], &[]),
            &workspace
        ),
        Err(CueCode::NotDeclared)
    );
}
