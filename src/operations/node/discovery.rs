use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::status::{PublicNodeConfig, load_node_config, public_config};
use crate::node::NodeContext;
use std::time::Duration;

pub fn public_discovery_status(
    handle: Option<&crate::discovery::DiscoveryStatusHandle>,
    include_addresses: bool,
) -> OperationResult<crate::discovery::DiscoveryStatus> {
    public_discovery_status_with_config(handle, include_addresses, None)
}

pub fn public_discovery_status_with_config(
    handle: Option<&crate::discovery::DiscoveryStatusHandle>,
    include_addresses: bool,
    config: Option<&PublicNodeConfig>,
) -> OperationResult<crate::discovery::DiscoveryStatus> {
    let Some(handle) = handle else {
        return Ok(configured_discovery_status(config));
    };
    handle
        .lock()
        .map_err(|_| {
            OperationError::new(OperationErrorCode::IoFailed, "discovery status unavailable")
        })
        .map(|mut snapshot| {
            snapshot.public_status(include_addresses, crate::util::time::unix_seconds())
        })
}

pub(super) fn configured_discovery_status(
    config: Option<&PublicNodeConfig>,
) -> crate::discovery::DiscoveryStatus {
    let settings = crate::domain::DiscoverySettings {
        enabled: config.is_some_and(|config| config.discovery_enabled),
        port: config
            .map(|config| config.discovery_port)
            .unwrap_or(crate::discovery::DISCOVERY_PORT),
        multicast_addr: crate::discovery::MULTICAST_GROUP.to_string(),
        broadcast: config.is_some_and(|config| config.discovery_broadcast),
    };
    crate::discovery::DiscoveryService::status_without_service(
        &settings,
        crate::discovery::platform_supported(),
        config.is_some_and(|config| config.discovery_secret_configured),
    )
}

pub fn scan_discovery(
    context: &NodeContext,
    scripts_dir: &std::path::Path,
    wait_seconds: u64,
    include_addresses: bool,
) -> OperationResult<crate::discovery::DiscoveryStatus> {
    let config = load_node_config(context)?;
    let direct_bind = config
        .network
        .direct_bind
        .as_deref()
        .map(str::parse::<std::net::SocketAddr>)
        .transpose()
        .map_err(|_| {
            OperationError::new(OperationErrorCode::InvalidInput, "direct bind is invalid")
        })?;
    if !config.discovery.enabled {
        let public_config = public_config(config.clone());
        return public_discovery_status_with_config(None, include_addresses, Some(&public_config));
    }
    let secret = if config.organization.discovery_secret_ref.is_empty() {
        None
    } else {
        let workspace = crate::workspace::Workspace::new(scripts_dir.to_path_buf());
        workspace.ensure_layout().map_err(|_| {
            OperationError::new(
                OperationErrorCode::DiscoveryInternal,
                "discovery workspace is unavailable",
            )
        })?;
        Some(
            crate::secrets::resolve_secret_value(
                &workspace,
                &config.organization.discovery_secret_ref,
                &crate::secrets::SecretAccess::allow_all(),
            )
            .map_err(|_| {
                OperationError::new(
                    OperationErrorCode::DiscoverySecretMismatch,
                    "discovery secret could not be resolved",
                )
            })?,
        )
    };
    let mut service = crate::discovery::DiscoveryService::start(
        config.discovery,
        context.clone(),
        direct_bind.map(|bind| bind.port()),
        secret,
    )
    .map_err(|error| match error {
        crate::discovery::DiscoveryError::UnsupportedPlatform => OperationError::new(
            OperationErrorCode::DiscoveryUnsupportedPlatform,
            "discovery is unsupported on this platform",
        ),
        crate::discovery::DiscoveryError::SecretInvalid => OperationError::new(
            OperationErrorCode::DiscoverySecretMismatch,
            "discovery secret is invalid",
        ),
        _ => OperationError::new(
            OperationErrorCode::DiscoveryInternal,
            "discovery could not start",
        ),
    })?;
    std::thread::sleep(Duration::from_secs(wait_seconds.clamp(1, 30)));
    let result = public_discovery_status(Some(&service.status()), include_addresses);
    service.stop();
    result
}
