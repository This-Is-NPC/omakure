use crate::cli::args::{NodeArgs, NodeCommand, NodeEnrollCommand};
use crate::cli::json;
use crate::domain::NodeConfig;
use crate::node::{NodeContext, NodeError, NodePathOverrides};
use crate::operations::node as node_ops;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use std::error::Error;
use std::fs;
use std::io::Read;
use std::time::Duration;

const CUE_ID_INVALID_MESSAGE: &str = "cue id must be 32 lowercase hexadecimal characters";

fn validate_cue_id(cue_id: Option<&str>) -> Result<(), OperationError> {
    if cue_id.is_some_and(|id| !crate::remote_cue::is_well_formed_cue_id(id)) {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            CUE_ID_INVALID_MESSAGE,
        ));
    }
    Ok(())
}

/// The local status read that explains a failed probe is a loopback lookup, not
/// a remote wait, so it gets a short budget of its own.
const SESSION_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Send one Cue, preferring the session this node's service already holds.
///
/// A separate process cannot dial a peer the service is connected to:
/// `register` refuses a second connection, and correctly so, because two
/// sessions with one peer would give the Health Plane two cursors for the same
/// node. So the running service is asked first, and the direct dial is the
/// fallback for the case it exists for -- no service, or a peer this node has
/// no standing session with.
///
/// `via` is reported so the path taken is visible rather than magic.
fn dispatch_cue(
    context: &NodeContext,
    scripts_dir: &std::path::Path,
    args: crate::cli::args::NodeCueArgs,
) -> OperationResult<serde_json::Value> {
    validate_cue_id(args.cue_id.as_deref())?;
    if !args.direct {
        match dispatch_cue_via_service(context, scripts_dir, &args) {
            Ok(data) => return Ok(data),
            // Two conditions fall through to the direct dial, and only two:
            // nothing is listening, and the service holds no session with this
            // peer. Any other refusal has been *decided* by the node that owns
            // the sessions, and dialling around it would be second-guessing it.
            Err(crate::cli::local_api::LocalApiError::Unreachable) => {}
            Err(crate::cli::local_api::LocalApiError::Refused(ref envelope))
                if envelope["error"]["code"] == "not_found" => {}
            Err(error) => {
                return Err(OperationError::new(
                    OperationErrorCode::InvalidInput,
                    error.to_string(),
                ))
            }
        }
    }
    crate::direct_service::dispatch_cue(
        args.endpoint,
        &args.peer_node_id,
        &args.script,
        &args.reason,
        args.wait_seconds,
        context,
        args.cue_id.as_deref(),
    )
    .map(|outcome| {
        serde_json::json!({
            "dispatched": true,
            "via": "direct",
            "cue_id": outcome.cue_id,
            "expected_run_id": outcome.expected_run_id,
            "answered": outcome.answered,
            "accepted": outcome.accepted,
            "code": outcome.code,
            "outcome_seen": outcome.outcome_seen,
        })
    })
    .map_err(map_direct_error)
}

/// Ask the running service to send it over the session it already holds.
fn dispatch_cue_via_service(
    context: &NodeContext,
    scripts_dir: &std::path::Path,
    args: &crate::cli::args::NodeCueArgs,
) -> Result<serde_json::Value, crate::cli::local_api::LocalApiError> {
    let Some(token) = std::env::var("OMAKURE_API_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
    else {
        return Err(crate::cli::local_api::LocalApiError::Unreachable);
    };
    let Some(bind) = node_api_bind(context, scripts_dir) else {
        return Err(crate::cli::local_api::LocalApiError::Unreachable);
    };
    let mut body = serde_json::json!({
        "peer_node_id": args.peer_node_id,
        "script": args.script,
        "reason": args.reason,
        "wait_seconds": args.wait_seconds,
    });
    if let Some(cue_id) = &args.cue_id {
        body["cue_id"] = serde_json::Value::String(cue_id.clone());
    }
    crate::cli::local_api::post_json(
        bind,
        &token,
        "/v1/node/cues",
        &body,
        crate::direct_service::dispatch_client_timeout(std::time::Duration::from_secs(u64::from(
            args.wait_seconds,
        ))),
    )
}

/// Create a publisher key, sign a baseline, or deliver one.
///
/// Delivery goes through the running service and only through it. The Cue path
/// keeps a direct dial for first contact, but a baseline has no first-contact
/// case: it goes to a Performer this node already conducts, which is exactly
/// the peer it already holds a session with. Adding a dial would have been a
/// second way into the responder for a case that does not exist.
fn dispatch_baseline(
    context: &NodeContext,
    scripts_dir: &std::path::Path,
    args: crate::cli::args::NodeBaselineArgs,
) -> OperationResult<serde_json::Value> {
    use crate::cli::args::NodeBaselineCommand;

    match args.command {
        NodeBaselineCommand::CreateKey => {
            let registry = node_ops::open_registry_for_baseline(context)?;
            let publisher =
                crate::baseline_publisher::BaselinePublisher::create(context, &registry).map_err(
                    |error| OperationError::new(OperationErrorCode::Conflict, error.to_string()),
                )?;
            Ok(serde_json::json!({
                "created": true,
                "key_id": hex_of(&publisher.key_id()),
                "public_key": hex_of(&publisher.public_key()),
            }))
        }
        NodeBaselineCommand::Publish(publish) => {
            let publisher = crate::baseline_publisher::BaselinePublisher::load_existing(context)
                .map_err(|error| {
                    OperationError::new(OperationErrorCode::NotFound, error.to_string())
                })?;
            let config = node_ops::load_node_config(context)?;
            let workspace = crate::workspace::Workspace::new(scripts_dir.to_path_buf());
            crate::operations::baseline::publish_baseline(
                &workspace,
                &publisher,
                &config.organization.id,
                &publish.scripts,
                unix_now(),
                publish.lifetime_seconds,
                &publish.out,
            )
            .map(|result| serde_json::to_value(result).expect("published baseline serializes"))
        }
        NodeBaselineCommand::Push(push) => {
            let workspace = crate::workspace::Workspace::new(scripts_dir.to_path_buf());
            let encoded = fs::read(&push.manifest).map_err(|error| {
                OperationError::new(
                    OperationErrorCode::NotFound,
                    format!("failed to read the manifest: {error}"),
                )
            })?;
            let manifest =
                crate::baseline::SignedBaselineManifest::decode(&encoded).map_err(|error| {
                    OperationError::new(OperationErrorCode::InvalidInput, error.to_string())
                })?;
            let bodies = crate::operations::baseline::bodies_for_manifest(&workspace, &manifest)?;
            push_baseline_via_service(context, scripts_dir, &push, &encoded, &bodies).map_err(
                |error| {
                    OperationError::new(
                        OperationErrorCode::InvalidInput,
                        format!(
                            "a baseline travels on the session this node's service holds, and \
                             that service could not be asked: {error}"
                        ),
                    )
                },
            )
        }
        NodeBaselineCommand::Rollback(rollback) => {
            let workspace = crate::workspace::Workspace::new(scripts_dir.to_path_buf());
            // The same policy the receive path reads, from this node's own
            // config, so a rollback can never be more permissive than the push
            // that installed the set would be if it arrived today.
            let policy = crate::baseline_push::read_policy(context);
            crate::operations::baseline::rollback_baseline(
                &workspace,
                &policy,
                rollback.confirmed,
                unix_now() as i64,
            )
            .map(|record| serde_json::to_value(record).expect("baseline record serializes"))
        }
    }
}

fn push_baseline_via_service(
    context: &NodeContext,
    scripts_dir: &std::path::Path,
    args: &crate::cli::args::NodeBaselinePushArgs,
    manifest: &[u8],
    bodies: &[Vec<u8>],
) -> Result<serde_json::Value, crate::cli::local_api::LocalApiError> {
    let Some(token) = std::env::var("OMAKURE_API_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return Err(crate::cli::local_api::LocalApiError::Unreachable);
    };
    let Some(bind) = node_api_bind(context, scripts_dir) else {
        return Err(crate::cli::local_api::LocalApiError::Unreachable);
    };
    crate::cli::local_api::post_json(
        bind,
        &token,
        "/v1/node/baselines",
        &serde_json::json!({
            "peer_node_id": args.peer_node_id,
            "manifest": hex_of(manifest),
            "scripts": bodies.iter().map(|body| hex_of(body)).collect::<Vec<_>>(),
            "wait_seconds": args.wait_seconds,
        }),
        crate::direct_service::dispatch_client_timeout(std::time::Duration::from_secs(u64::from(
            args.wait_seconds,
        ))),
    )
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Probe one peer, and name the one cause the prober cannot see for itself.
///
/// Unlike a Cue, a probe is deliberately *not* relayed through the running
/// service. A Cue is an instruction that has to arrive, so the session the
/// service holds is the only way to deliver it. A probe is a question, and for
/// a peer this node already has a session with the answer is already in hand:
/// that session was built by the same handshake, identity check, and
/// authorization a probe performs, and it is torn down when any of them stops
/// holding. Relaying would also write a `probe_accepted` audit row for a
/// handshake that never happened, which is worse than no answer.
///
/// What the prober cannot see is why it was refused. A peer that already holds
/// a session with this node rejects the second connection inside `register`
/// and hangs up without a reply, so all the dial observes is a closed stream:
/// `transport_internal`, "direct transport I/O failed". The reason lives on
/// the other side of the wire, but the *fact* is local -- this node's own
/// service knows which peers it is connected to -- so it is read from there.
fn dispatch_direct_probe(
    context: &NodeContext,
    scripts_dir: &std::path::Path,
    args: crate::cli::args::NodeDirectProbeArgs,
) -> OperationResult<serde_json::Value> {
    let error = match crate::direct_service::probe(args.endpoint, &args.peer_node_id, context) {
        Ok(()) => return Ok(serde_json::json!({"accepted": true})),
        Err(error) => error,
    };
    if hung_up_mid_session(&error)
        && service_holds_session(context, scripts_dir, &args.peer_node_id)
    {
        return Err(OperationError::new(
            OperationErrorCode::AlreadyExists,
            format!(
                "this node's service already holds a session with {}, and a peer accepts \
                 only one; that session is itself the proof a probe would produce, so read \
                 it from `node status` instead of dialling a second time",
                args.peer_node_id
            ),
        ));
    }
    Err(map_direct_error(error))
}

/// Whether the peer accepted the connection and then dropped it without
/// answering, which is what a refusal inside `register` looks like from here.
///
/// A refused or unanswered *connection* is excluded deliberately. It reaches
/// the caller as the same `Io` variant, but it means nothing is listening at
/// that address -- a wrong endpoint, not a duplicate session -- and reporting a
/// standing session for it would hide the real fault.
fn hung_up_mid_session(error: &crate::direct_service::DirectServiceError) -> bool {
    let crate::direct_service::DirectServiceError::Io(error) = error else {
        return false;
    };
    matches!(
        error.kind(),
        std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
    )
}

/// Whether this node's own service reports a live session with `peer`.
///
/// Read at most once, only after a probe has already failed, so the ordinary
/// path pays nothing for it. Any inability to ask -- no token, no service, no
/// answer -- means the failure is reported exactly as it arrived rather than
/// guessed at.
fn service_holds_session(context: &NodeContext, scripts_dir: &std::path::Path, peer: &str) -> bool {
    let Ok(token) = std::env::var("OMAKURE_API_TOKEN") else {
        return false;
    };
    let Some(addr) = node_api_bind(context, scripts_dir) else {
        return false;
    };
    if token.is_empty() {
        return false;
    }
    let Ok(data) =
        crate::cli::local_api::get_json(addr, &token, "/v1/node/status", SESSION_LOOKUP_TIMEOUT)
    else {
        return false;
    };
    data["transport"]["peers"].as_array().is_some_and(|peers| {
        peers
            .iter()
            .any(|entry| entry["node_id"] == peer && entry["state"] == "connected")
    })
}

/// Where this node's running service can be reached.
///
/// The service records this itself, because `api.bind` in the config is only a
/// request — `node serve --bind` wins over it, and a process reading the config
/// alone would look in the wrong place. The config is still the fallback for a
/// service started before this file existed.
fn node_api_bind(
    context: &NodeContext,
    scripts_dir: &std::path::Path,
) -> Option<std::net::SocketAddr> {
    let workspace = crate::workspace::Workspace::new(scripts_dir.to_path_buf());
    if let Ok(recorded) = fs::read_to_string(workspace.service_endpoint_path()) {
        if let Some(addr) = serde_json::from_str::<serde_json::Value>(&recorded)
            .ok()
            .and_then(|value| {
                value
                    .get("api_bind")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|bind| bind.parse::<std::net::SocketAddr>().ok())
            })
        {
            return Some(addr);
        }
    }
    let mut file = context.open_public_file().ok()??;
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
    NodeConfig::parse(&contents)
        .ok()?
        .api
        .bind
        .parse::<std::net::SocketAddr>()
        .ok()
}

pub fn run(
    scripts_dir: std::path::PathBuf,
    args: NodeArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let context =
        match NodeContext::resolve(NodePathOverrides::new(args.state_dir, args.config_path)) {
            Ok(context) => context,
            Err(error) => return emit_error(json_output, map_node_error(error)),
        };
    let result: OperationResult<serde_json::Value> = match args.command {
        NodeCommand::Serve(args) => {
            return crate::cli::node_service::run(scripts_dir, context, args);
        }
        NodeCommand::DirectProbe(args) => dispatch_direct_probe(&context, &scripts_dir, args),
        // Prefers `POST /v1/node/cues` on the running service, because a
        // separate process cannot dial a peer that service already has a
        // session with. The route is not a second authorization surface for the
        // Cue: every gate is on the receiving node, and `node:write` decides
        // only whether this operator may ask.
        NodeCommand::Cue(args) => dispatch_cue(&context, &scripts_dir, args),
        NodeCommand::Baseline(args) => dispatch_baseline(&context, &scripts_dir, args),
        NodeCommand::Authority(args) => match args.command {
            crate::cli::args::NodeAuthorityCommand::Create(args) => {
                node_ops::create_enrollment_authority(&context, args.confirmed)
                    .map(|result| serde_json::to_value(result).expect("authority serializes"))
            }
            crate::cli::args::NodeAuthorityCommand::Show => {
                node_ops::read_enrollment_authority(&context)
                    .map(|result| serde_json::to_value(result).expect("authority serializes"))
            }
            crate::cli::args::NodeAuthorityCommand::Issue(args) => {
                node_ops::issue_enrollment_bundle(
                    &context,
                    node_ops::BundleIssueRequest {
                        audience_node_id: args.audience,
                        role: args.role,
                        capabilities: args.capabilities,
                        lifetime_seconds: args.lifetime_seconds,
                    },
                )
                .map(|result| serde_json::to_value(result).expect("bundle serializes"))
            }
        },
        NodeCommand::Init => {
            node_ops::initialize_node_nonblocking(&context, &NodeConfig::default())
                .map(|result| serde_json::to_value(result).expect("node initialization serializes"))
        }
        NodeCommand::Status => node_ops::public_node_status(&context)
            .map(|result| serde_json::to_value(result).expect("node status serializes")),
        NodeCommand::Peers => node_ops::list_trusted_peers(&context)
            .map(|result| serde_json::to_value(result).expect("peer list serializes")),
        // Thin adapter: the protocol-neutral operation decides everything and
        // this arm only renders it. The identical value backs `GET /v1/node/health`.
        NodeCommand::Health => crate::operations::health::open_observational_registry(&context)
            .and_then(|registry| crate::operations::health::fleet_status(&registry))
            .map(|result| serde_json::to_value(result).expect("fleet status serializes")),
        // Thin adapter, same shape: the bounded Signal feed is decided by the
        // protocol-neutral operation and only rendered here. The identical
        // value backs `GET /v1/node/signals`.
        NodeCommand::Signals => crate::operations::health::open_observational_registry(&context)
            .and_then(|registry| crate::operations::health::signal_feed(&registry))
            .map(|result| serde_json::to_value(result).expect("signal feed serializes")),
        NodeCommand::Discovery(args) => node_ops::scan_discovery(
            &context,
            &scripts_dir,
            args.wait_seconds,
            args.include_addresses,
        )
        .map(|result| serde_json::to_value(result).expect("discovery status serializes")),
        NodeCommand::Trust(args) => node_ops::import_manual_trust(
            &context,
            node_ops::ManualTrustRequest {
                node_id: args.node_id,
                public_key: args.public_key,
                transport_certificate: args.transport_certificate,
                role: args.role,
                capabilities: args.capabilities,
                actor: args.actor,
                reason: args.reason,
                confirmed: args.confirmed,
            },
        )
        .map(|result| serde_json::to_value(result).expect("peer serializes")),
        NodeCommand::Enroll(args) => match args.command {
            NodeEnrollCommand::Request(args) => node_ops::request_manual_enrollment(
                &context,
                args.endpoint,
                &args.role,
                args.capabilities,
                args.lifetime_seconds,
            )
            .map(|result| serde_json::to_value(result).expect("enrollment serializes")),
            NodeEnrollCommand::Approve(args) => node_ops::approve_manual_enrollment(
                &context,
                node_ops::ManualEnrollmentApprovalRequest {
                    request_hex: args.request_hex,
                    transport_certificate: args.transport_certificate,
                    code: args.code,
                    actor: args.actor,
                    reason: args.reason,
                    confirmed: args.confirmed,
                    expected_node_id: None,
                },
            )
            .map(|result| serde_json::to_value(result).expect("peer serializes")),
            NodeEnrollCommand::Reject(args) => node_ops::reject_manual_enrollment(
                &context,
                node_ops::ManualEnrollmentRejectionRequest {
                    node_id: args.node_id,
                    actor: args.actor,
                    reason: args.reason,
                    confirmed: args.confirmed,
                },
            )
            .map(|result| serde_json::to_value(result).expect("peer serializes")),
            NodeEnrollCommand::Apply(args) => apply_bundle_inputs(args)
                .and_then(|request| node_ops::apply_signed_bundle(&context, request))
                .map(|result| serde_json::to_value(result).expect("peer serializes")),
        },
        NodeCommand::Capabilities(args) => node_ops::update_peer_capabilities(
            &context,
            node_ops::CapabilityUpdateRequest {
                node_id: args.node_id,
                capabilities: args.capabilities,
                actor: args.actor,
                reason: args.reason,
                confirmed: args.confirmed,
            },
        )
        .map(|result| serde_json::to_value(result).expect("peer serializes")),
        NodeCommand::Revoke(args) => node_ops::revoke_peer(
            &context,
            &crate::workspace::Workspace::new(scripts_dir.clone()),
            node_ops::RevocationRequest {
                node_id: args.node_id,
                actor: args.actor,
                reason: args.reason,
                confirmed: args.confirmed,
            },
        )
        .map(|result| serde_json::to_value(result).expect("peer serializes")),
        NodeCommand::Reset(args) => node_ops::reset_node(&context, args.confirmed)
            .map(|result| serde_json::to_value(result).expect("node reset serializes")),
    };
    emit_result(json_output, result)
}

fn apply_bundle_inputs(
    args: crate::cli::args::NodeEnrollApplyArgs,
) -> OperationResult<node_ops::SignedBundleApplyRequest> {
    let bundle = read_bounded_file(&args.bundle_file, crate::enrollment::MAX_BUNDLE_INPUT_BYTES)
        .map_err(|_| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                "signed enrollment bundle could not be read",
            )
        })?;
    let bundle_hex = if bundle.len().is_multiple_of(2)
        && bundle
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        String::from_utf8(bundle).map_err(|_| {
            OperationError::new(
                OperationErrorCode::EnrollmentInvalid,
                "signed enrollment bundle is invalid",
            )
        })?
    } else {
        crate::enrollment::hex_bytes(&bundle)
    };
    Ok(node_ops::SignedBundleApplyRequest {
        bundle_hex,
        bootstrap_token: String::new(),
        bootstrap_nonce: args.bootstrap_nonce,
        bootstrap_token_path: Some(args.bootstrap_token_file),
    })
}

fn read_bounded_file(path: &std::path::Path, max_bytes: usize) -> std::io::Result<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > max_bytes as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file exceeds the configured bound",
        ));
    }
    let file = fs::File::open(path)?;
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes as u64 + 1).read_to_end(&mut contents)?;
    if contents.len() > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file exceeds the configured bound",
        ));
    }
    Ok(contents)
}

fn map_direct_error(error: crate::direct_service::DirectServiceError) -> OperationError {
    let code = match &error {
        crate::direct_service::DirectServiceError::Protocol(error) => match error.code() {
            crate::direct_transport::ProtocolErrorCode::UnsupportedVersion => {
                OperationErrorCode::TransportUnsupportedVersion
            }
            crate::direct_transport::ProtocolErrorCode::InvalidFrame => {
                OperationErrorCode::TransportInvalidFrame
            }
            crate::direct_transport::ProtocolErrorCode::MessageTooLarge => {
                OperationErrorCode::TransportMessageTooLarge
            }
            crate::direct_transport::ProtocolErrorCode::HandshakeFailed => {
                OperationErrorCode::TransportHandshakeFailed
            }
            crate::direct_transport::ProtocolErrorCode::IdentityMismatch => {
                OperationErrorCode::TransportIdentityMismatch
            }
            crate::direct_transport::ProtocolErrorCode::NotEnrolled => {
                OperationErrorCode::TransportNotEnrolled
            }
            crate::direct_transport::ProtocolErrorCode::Revoked => {
                OperationErrorCode::TransportRevoked
            }
            crate::direct_transport::ProtocolErrorCode::Expired => {
                OperationErrorCode::TransportExpired
            }
            crate::direct_transport::ProtocolErrorCode::Replay => {
                OperationErrorCode::TransportReplay
            }
            crate::direct_transport::ProtocolErrorCode::RateLimited => {
                OperationErrorCode::TransportRateLimited
            }
            crate::direct_transport::ProtocolErrorCode::Internal => {
                OperationErrorCode::TransportInternal
            }
        },
        _ => OperationErrorCode::TransportInternal,
    };
    OperationError::new(code, error.to_string())
}

fn emit_result<T: serde::Serialize>(
    json_output: bool,
    result: OperationResult<T>,
) -> Result<(), Box<dyn Error>> {
    match result {
        Ok(data) => {
            if json_output {
                json::print_ok(data);
            } else {
                println!("{}", serde_json::to_string_pretty(&data)?);
            }
            Ok(())
        }
        Err(error) => emit_error(json_output, error),
    }
}

fn emit_error(json_output: bool, error: OperationError) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(error.code.as_str(), error.message);
        std::process::exit(1);
    }
    Err(error.to_string().into())
}

fn map_node_error(error: NodeError) -> OperationError {
    match error {
        NodeError::InvalidPath { .. }
        | NodeError::IncompleteTestOverrides
        | NodeError::TestOverrideOutsideTestMode
        | NodeError::TestModeUnavailable => {
            OperationError::new(OperationErrorCode::InvalidInput, error.to_string())
        }
        NodeError::Config(error) => {
            OperationError::new(OperationErrorCode::InvalidInput, error.to_string())
        }
        NodeError::InsecurePath(_) => {
            OperationError::new(OperationErrorCode::RegistryInvalid, error.to_string())
        }
        NodeError::UnsafePath(_)
        | NodeError::UnexpectedFileType(_)
        | NodeError::ExistingConfig(_) => {
            OperationError::new(OperationErrorCode::RegistryInvalid, error.to_string())
        }
        NodeError::LifecycleBusy => OperationError::new(
            OperationErrorCode::Conflict,
            "node service is active; stop it before resetting",
        ),
        NodeError::Io(error) => {
            OperationError::new(OperationErrorCode::IoFailed, error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    /// No wait-bounded command may set its own client timeout.
    ///
    /// `--wait-seconds` is the budget the *peer* is given to answer in. The
    /// session thread waits a little longer than that before answering
    /// `answered: false` itself, because silence is a designed verdict here and
    /// something has to turn it into one. A client that closes the socket at
    /// exactly `--wait-seconds` closes it before that verdict exists, and the
    /// operator sees a transport error where the protocol has an answer -- the
    /// precise case `answered: false` was built to report. The two timeouts are
    /// derived from one rule so they cannot drift apart again.
    #[test]
    fn no_wait_bounded_command_gives_up_before_the_service_answers() {
        let source = include_str!("node.rs");
        assert!(
            // Split so this test does not match its own source.
            !source.contains(&format!(
                "Duration::from_secs(u64::from(args.{}))",
                "wait_seconds"
            )),
            "a wait-bounded command is timing its client on the peer's budget \
             instead of on the service's own answer deadline"
        );
        assert_eq!(
            source
                .matches(&format!(
                    "crate::direct_service::{}(",
                    "dispatch_client_timeout"
                ))
                .count(),
            2,
            "the Cue and baseline pushes are the two wait-bounded commands that \
             call this node's own service; a third one must derive its timeout \
             the same way"
        );
    }
}
