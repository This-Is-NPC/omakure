use crate::cli::args::{NodeArgs, NodeCommand};
use crate::cli::json;
use crate::domain::NodeConfig;
use crate::node::{NodeContext, NodeError, NodePathOverrides};
use crate::operations::node as node_ops;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use std::error::Error;

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
        NodeCommand::Init => {
            node_ops::initialize_node_nonblocking(&context, &NodeConfig::default())
                .map(|result| serde_json::to_value(result).expect("node initialization serializes"))
        }
        NodeCommand::Status => node_ops::public_node_status(&context)
            .map(|result| serde_json::to_value(result).expect("node status serializes")),
        NodeCommand::Peers => node_ops::list_trusted_peers(&context)
            .map(|result| serde_json::to_value(result).expect("peer list serializes")),
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
