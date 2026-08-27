use crate::cli::args::{NodeArgs, NodeCommand, NodeEnrollCommand};
use crate::cli::json;
use crate::domain::NodeConfig;
use crate::node::{NodeContext, NodeError, NodePathOverrides};
use crate::operations::node as node_ops;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use std::error::Error;
use std::fs;
use std::io::Read;

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
    crate::cli::local_api::post_json(
        bind,
        &token,
        "/v1/node/cues",
        &serde_json::json!({
            "peer_node_id": args.peer_node_id,
            "script": args.script,
            "reason": args.reason,
            "wait_seconds": args.wait_seconds,
        }),
        std::time::Duration::from_secs(u64::from(args.wait_seconds)),
    )
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
        NodeCommand::DirectProbe(args) => {
            crate::direct_service::probe(args.endpoint, &args.peer_node_id, &context)
                .map(|()| serde_json::json!({"accepted": true}))
                .map_err(map_direct_error)
        }
        // Prefers `POST /v1/node/cues` on the running service, because a
        // separate process cannot dial a peer that service already has a
        // session with. The route is not a second authorization surface for the
        // Cue: every gate is on the receiving node, and `node:write` decides
        // only whether this operator may ask.
        NodeCommand::Cue(args) => dispatch_cue(&context, &scripts_dir, args),
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
        NodeCommand::Health => crate::operations::health::fleet_status(&context)
            .map(|result| serde_json::to_value(result).expect("fleet status serializes")),
        // Thin adapter, same shape: the bounded Signal feed is decided by the
        // protocol-neutral operation and only rendered here. The identical
        // value backs `GET /v1/node/signals`.
        NodeCommand::Signals => crate::operations::health::signal_feed(&context)
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
        NodeError::UnsafePath(_)
        | NodeError::UnexpectedFileType(_)
        | NodeError::InsecurePath(_)
        | NodeError::ExistingConfig(_) => OperationError::new(
            OperationErrorCode::RegistryInvalid,
            "node state is invalid or insecure",
        ),
        NodeError::LifecycleBusy => OperationError::new(
            OperationErrorCode::Conflict,
            "node service is active; stop it before resetting",
        ),
        NodeError::Io(error) => {
            OperationError::new(OperationErrorCode::IoFailed, error.to_string())
        }
    }
}
