use crate::cli::args::{NodeArgs, NodeCommand, NodeEnrollCommand};
use crate::cli::json;
use crate::domain::NodeConfig;
use crate::node::{NodeContext, NodeError, NodePathOverrides};
use crate::operations::node as node_ops;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use std::error::Error;
use std::fs;
use std::io::Read;

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
        // Deliberately CLI-only. `.docs/cli-http-parity.md` records the status;
        // an HTTP route would be a fifth authorization surface to keep in step
        // for no safety gain, ahead of a feature whose whole point is bounding
        // what a remote caller can reach.
        NodeCommand::Cue(args) => crate::direct_service::dispatch_cue(
            args.endpoint,
            &args.peer_node_id,
            &args.script,
            &args.reason,
            &context,
        )
        // `answered` and `accepted` are reported apart because a Performer
        // that refuses on trust, role, or capability says nothing, and
        // "no answer" must not be printed as a verdict.
        .map(|outcome| {
            serde_json::json!({
                "dispatched": true,
                "cue_id": outcome.cue_id,
                "expected_run_id": outcome.expected_run_id,
                "answered": outcome.answered,
                "accepted": outcome.accepted,
                "code": outcome.code,
            })
        })
        .map_err(map_direct_error),
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
