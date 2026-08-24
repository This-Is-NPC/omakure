use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::str::FromStr;
use thiserror::Error;

pub const NODE_CONFIG_VERSION: u8 = 1;
pub const MAX_MESSAGE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_NODE_CONFIG_RELAYS: usize = 32;
pub const MAX_NODE_CONFIG_STATIC_PEERS: usize = 256;
pub const MAX_NODE_CONFIG_RELAY_BYTES: usize = 512;
pub const MAX_NODE_CONFIG_STATIC_PEER_BYTES: usize = 256;
pub const MAX_NODE_CONFIG_SECRET_REF_BYTES: usize = 256;
const MAX_BIND_BYTES: usize = 128;
const MAX_NETWORK_MODE_BYTES: usize = 64;
const MAX_ENROLLMENT_BYTES: usize = 64;
const MAX_DISPLAY_NAME_BYTES: usize = 128;
const MAX_ORGANIZATION_ID_BYTES: usize = 128;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NodeConfigError {
    #[error("node.toml parse error: {0}")]
    Parse(String),
    #[error("unsupported node.toml version: {0}")]
    UnsupportedVersion(u8),
    #[error("invalid node.toml: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    pub version: u8,
    pub node: NodeSettings,
    pub api: ApiSettings,
    pub network: NetworkSettings,
    pub trust: TrustSettings,
    pub organization: OrganizationSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSettings {
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSettings {
    pub bind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkSettings {
    pub mode: String,
    pub relays: Vec<String>,
    pub static_peers: Vec<String>,
    pub max_message_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustSettings {
    pub enrollment: String,
    pub allow_remote_cues: bool,
    pub allow_baseline_push: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationSettings {
    pub id: String,
    pub discovery_secret_ref: String,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            version: NODE_CONFIG_VERSION,
            node: NodeSettings {
                display_name: String::new(),
            },
            api: ApiSettings {
                bind: "127.0.0.1:7878".to_string(),
            },
            network: NetworkSettings {
                mode: "direct".to_string(),
                relays: Vec::new(),
                static_peers: Vec::new(),
                max_message_bytes: 1_048_576,
            },
            trust: TrustSettings {
                enrollment: "disabled".to_string(),
                allow_remote_cues: false,
                allow_baseline_push: false,
            },
            organization: OrganizationSettings {
                id: String::new(),
                discovery_secret_ref: String::new(),
            },
        }
    }
}

impl NodeConfig {
    pub fn parse(text: &str) -> Result<Self, NodeConfigError> {
        let config: Self =
            toml::from_str(text).map_err(|err| NodeConfigError::Parse(err.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), NodeConfigError> {
        if self.version != NODE_CONFIG_VERSION {
            return Err(NodeConfigError::UnsupportedVersion(self.version));
        }

        validate_text(
            "node.display_name",
            &self.node.display_name,
            MAX_DISPLAY_NAME_BYTES,
            true,
        )?;
        validate_text(
            "organization.id",
            &self.organization.id,
            MAX_ORGANIZATION_ID_BYTES,
            true,
        )?;
        validate_text("api.bind", &self.api.bind, MAX_BIND_BYTES, false)?;
        validate_bind(&self.api.bind)?;

        validate_text(
            "network.mode",
            &self.network.mode,
            MAX_NETWORK_MODE_BYTES,
            false,
        )?;

        match self.network.mode.as_str() {
            "direct" | "direct-with-nostr-fallback" | "nostr" => {}
            value => {
                return Err(NodeConfigError::Invalid(format!(
                    "network.mode `{value}` is invalid"
                )))
            }
        }
        if self.network.mode == "direct" && !self.network.relays.is_empty() {
            return Err(NodeConfigError::Invalid(
                "network.relays must be empty in direct mode".to_string(),
            ));
        }
        if self.network.relays.len() > MAX_NODE_CONFIG_RELAYS {
            return Err(NodeConfigError::Invalid(
                "network.relays has too many entries".to_string(),
            ));
        }
        if self.network.static_peers.len() > MAX_NODE_CONFIG_STATIC_PEERS {
            return Err(NodeConfigError::Invalid(
                "network.static_peers has too many entries".to_string(),
            ));
        }
        if !(1..=MAX_MESSAGE_BYTES).contains(&self.network.max_message_bytes) {
            return Err(NodeConfigError::Invalid(format!(
                "network.max_message_bytes must be between 1 and {MAX_MESSAGE_BYTES}"
            )));
        }
        for relay in &self.network.relays {
            validate_relay(relay)?;
        }
        for peer in &self.network.static_peers {
            validate_static_peer(peer)?;
        }

        validate_text(
            "trust.enrollment",
            &self.trust.enrollment,
            MAX_ENROLLMENT_BYTES,
            false,
        )?;
        match self.trust.enrollment.as_str() {
            "disabled" | "manual" | "signed-bundle" => {}
            value => {
                return Err(NodeConfigError::Invalid(format!(
                    "trust.enrollment `{value}` is invalid"
                )))
            }
        }
        if self.trust.enrollment == "disabled"
            && (self.trust.allow_remote_cues || self.trust.allow_baseline_push)
        {
            return Err(NodeConfigError::Invalid(
                "remote capabilities require enrollment to be enabled".to_string(),
            ));
        }
        validate_secret_ref(&self.organization.discovery_secret_ref)?;
        Ok(())
    }

    pub fn to_toml(&self) -> Result<String, NodeConfigError> {
        self.validate()?;
        toml::to_string_pretty(self).map_err(|err| NodeConfigError::Parse(err.to_string()))
    }
}

pub fn parse_node_config(text: &str) -> Result<NodeConfig, NodeConfigError> {
    NodeConfig::parse(text)
}

fn validate_text(
    field: &str,
    value: &str,
    max_bytes: usize,
    empty_allowed: bool,
) -> Result<(), NodeConfigError> {
    if !empty_allowed && value.is_empty() {
        return Err(NodeConfigError::Invalid(format!(
            "{field} must not be empty"
        )));
    }
    if value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(NodeConfigError::Invalid(format!("{field} is invalid")));
    }
    Ok(())
}

fn validate_bind(value: &str) -> Result<(), NodeConfigError> {
    let address = SocketAddr::from_str(value)
        .map_err(|_| NodeConfigError::Invalid("api.bind must be a socket address".to_string()))?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err(NodeConfigError::Invalid(
            "api.bind must use a loopback address and a non-zero port".to_string(),
        ));
    }
    Ok(())
}

fn validate_relay(value: &str) -> Result<(), NodeConfigError> {
    if value.len() > MAX_NODE_CONFIG_RELAY_BYTES {
        return Err(NodeConfigError::Invalid(
            "network relay is too long".to_string(),
        ));
    }
    let Some(rest) = value.strip_prefix("wss://") else {
        return Err(NodeConfigError::Invalid(format!(
            "network relay `{value}` must use wss://"
        )));
    };
    if rest.is_empty() || rest.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(NodeConfigError::Invalid(format!(
            "network relay `{value}` is invalid"
        )));
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') || authority.contains('#') {
        return Err(NodeConfigError::Invalid(format!(
            "network relay `{value}` is invalid"
        )));
    }
    validate_relay_authority(authority).map_err(|reason| {
        NodeConfigError::Invalid(format!("network relay `{value}` is invalid: {reason}"))
    })?;
    if rest[authority_end..].contains('#') {
        return Err(NodeConfigError::Invalid(format!(
            "network relay `{value}` has a fragment"
        )));
    }
    Ok(())
}

fn validate_static_peer(value: &str) -> Result<(), NodeConfigError> {
    if value.len() > MAX_NODE_CONFIG_STATIC_PEER_BYTES {
        return Err(NodeConfigError::Invalid(
            "static peer is too long".to_string(),
        ));
    }
    let Some((node_id, endpoint)) = value.split_once('@') else {
        return Err(NodeConfigError::Invalid(format!(
            "static peer `{value}` must be node_id@host:port"
        )));
    };
    if !is_node_id(node_id) {
        return Err(NodeConfigError::Invalid(format!(
            "static peer `{value}` has an invalid node id"
        )));
    }
    validate_host_port(endpoint, true).map_err(|reason| {
        NodeConfigError::Invalid(format!("static peer `{value}` is invalid: {reason}"))
    })
}

fn validate_relay_authority(value: &str) -> Result<(), &'static str> {
    if value.starts_with('[') {
        let close = value.find(']').ok_or("missing IPv6 bracket")?;
        let host = &value[1..close];
        if host.is_empty() || host.contains(['[', ']']) {
            return Err("invalid host");
        }
        let suffix = &value[close + 1..];
        if !suffix.is_empty() {
            let port = suffix.strip_prefix(':').ok_or("invalid port")?;
            if port.parse::<u16>().ok().filter(|port| *port != 0).is_none() {
                return Err("invalid port");
            }
        }
        return Ok(());
    }
    if value.contains(':') {
        return validate_host_port(value, false);
    }
    validate_host(value)
}

fn validate_host(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b'/' || byte == b'@')
    {
        return Err("invalid host");
    }
    Ok(())
}

fn validate_host_port(value: &str, allow_bracketed_ipv6: bool) -> Result<(), &'static str> {
    let (host, port) = if allow_bracketed_ipv6 && value.starts_with('[') {
        let close = value.find(']').ok_or("missing IPv6 bracket")?;
        let host = &value[1..close];
        let port = value
            .get(close + 1..)
            .and_then(|suffix| suffix.strip_prefix(':'))
            .ok_or("missing port")?;
        if host.is_empty() || host.contains(['[', ']']) {
            return Err("invalid host");
        }
        (host, port)
    } else {
        let separator = value.rfind(':').ok_or("missing port")?;
        let host = &value[..separator];
        let port = &value[separator + 1..];
        if host.is_empty() || host.contains(':') {
            return Err("invalid host");
        }
        (host, port)
    };
    let port: u16 = port.parse().map_err(|_| "invalid port")?;
    if port == 0 {
        return Err("port must be non-zero");
    }
    if host
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'/' || byte == b'@')
    {
        return Err("invalid host");
    }
    Ok(())
}

fn is_node_id(value: &str) -> bool {
    value.len() == 69
        && value.starts_with("omk1_")
        && value[5..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_secret_ref(value: &str) -> Result<(), NodeConfigError> {
    if value.len() > MAX_NODE_CONFIG_SECRET_REF_BYTES {
        return Err(NodeConfigError::Invalid(
            "organization.discovery_secret_ref is invalid".to_string(),
        ));
    }
    if value.is_empty() {
        return Ok(());
    }
    let Some(rest) = value.strip_prefix("secret://") else {
        return Err(NodeConfigError::Invalid(
            "organization.discovery_secret_ref must be empty or secret://provider/name".to_string(),
        ));
    };
    let Some((provider, name)) = rest.split_once('/') else {
        return Err(NodeConfigError::Invalid(
            "organization.discovery_secret_ref must be secret://provider/name".to_string(),
        ));
    };
    if provider.is_empty()
        || name.is_empty()
        || name.contains('/')
        || !is_ref_component(provider)
        || !is_ref_component(name)
    {
        return Err(NodeConfigError::Invalid(
            "organization.discovery_secret_ref is invalid".to_string(),
        ));
    }
    Ok(())
}

fn is_ref_component(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_toml() -> String {
        NodeConfig::default().to_toml().unwrap()
    }

    #[test]
    fn default_config_is_the_frozen_safe_baseline() {
        let config = NodeConfig::parse(&valid_toml()).unwrap();
        assert_eq!(config, NodeConfig::default());
    }

    #[test]
    fn strict_parser_rejects_unknown_missing_duplicate_and_future_fields() {
        let base = valid_toml();
        for input in [
            base.replace("[node]", "extra = true\n\n[node]"),
            base.replace("display_name = \"\"", "# display_name omitted"),
            format!("{base}\nversion = 1\n"),
            base.replace("version = 1", "version = 2"),
        ] {
            assert!(NodeConfig::parse(&input).is_err(), "accepted: {input}");
        }
    }

    #[test]
    fn parser_rejects_invalid_types_and_trailing_data() {
        assert!(
            NodeConfig::parse(&valid_toml().replace("version = 1", "version = \"1\"")).is_err()
        );
        assert!(NodeConfig::parse(&format!("{}\nnot =", valid_toml())).is_err());
    }

    #[test]
    fn validation_rejects_unsafe_network_and_secret_values() {
        let mut config = NodeConfig::default();
        config.api.bind = "0.0.0.0:7878".into();
        assert!(config.validate().is_err());
        config = NodeConfig::default();
        config.network.static_peers = vec!["omk1_00@host:1".into()];
        assert!(config.validate().is_err());
        config = NodeConfig::default();
        config.organization.discovery_secret_ref = "secret://prod/raw/value".into();
        assert!(config.validate().is_err());
        config.organization.discovery_secret_ref = "plain-secret-value".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validation_accepts_canonical_peer_relay_and_secret_ref() {
        let mut config = NodeConfig::default();
        config.network.mode = "nostr".into();
        config.network.relays = vec!["wss://relay.example.test/path".into()];
        config.network.static_peers = vec![format!("omk1_{}@127.0.0.1:7879", "a".repeat(64))];
        config.organization.discovery_secret_ref = "secret://prod/discovery_key".into();
        config.trust.enrollment = "manual".into();
        config.validate().unwrap();
    }

    #[test]
    fn validation_rejects_unbounded_public_config_values() {
        let mut config = NodeConfig::default();
        config.node.display_name = "d".repeat(129);
        assert!(config.validate().is_err());

        config = NodeConfig::default();
        config.organization.id = "o".repeat(129);
        assert!(config.validate().is_err());

        config = NodeConfig::default();
        config.network.mode = "nostr".into();
        config.network.relays = vec!["wss://relay.example.test".into(); MAX_NODE_CONFIG_RELAYS + 1];
        assert!(config.validate().is_err());

        config.network.relays.clear();
        config.network.static_peers = vec![
            format!("omk1_{}@127.0.0.1:7879", "a".repeat(64));
            MAX_NODE_CONFIG_STATIC_PEERS + 1
        ];
        assert!(config.validate().is_err());

        config.network.static_peers.clear();
        config.network.relays = vec![format!("wss://{}", "r".repeat(MAX_NODE_CONFIG_RELAY_BYTES))];
        assert!(config.validate().is_err());

        config.network.relays.clear();
        config.organization.discovery_secret_ref =
            "secret://".to_string() + &"provider".repeat(MAX_NODE_CONFIG_SECRET_REF_BYTES);
        assert!(config.validate().is_err());
    }

    #[test]
    fn public_model_contains_no_identity_or_resolved_secret_fields() {
        let text = format!("{:?}", NodeConfig::default());
        assert!(!text.contains("identity"));
        assert!(!text.contains("private"));
        assert!(!text.contains("resolved"));
    }
}
