use super::node_id::is_node_id;
use crate::util::hex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str::FromStr;
use thiserror::Error;

pub const NODE_CONFIG_VERSION: u8 = 1;
pub const MAX_NODE_CONFIG_STATIC_PEERS: usize = 256;
pub const MAX_NODE_CONFIG_STATIC_PEER_BYTES: usize = 256;
pub const MAX_NODE_CONFIG_SECRET_REF_BYTES: usize = 256;
const MAX_BIND_BYTES: usize = 128;
const MAX_ENROLLMENT_BYTES: usize = 64;
const MAX_DISPLAY_NAME_BYTES: usize = 128;
const MAX_ORGANIZATION_ID_BYTES: usize = 128;
const MAX_AUTHORITY_KEYS: usize = 64;

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
    #[serde(default)]
    pub discovery: DiscoverySettings,
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
    pub static_peers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_bind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustSettings {
    pub enrollment: String,
    pub allow_remote_cues: bool,
    /// The scripts this node will run on another node's orders.
    ///
    /// Declarative and deny-by-default: empty or absent means nothing is
    /// remotely executable, even with `allow_remote_cues = true`. Two
    /// independent switches, both of which must be set deliberately.
    ///
    /// Without this, "what may run remotely" would be every discoverable
    /// script minus `.omakureignore` — allow-by-default, whose failure mode is
    /// silent: a new file in the workspace would become remotely executable
    /// with nobody having declared it.
    #[serde(default)]
    pub remote_cue_scripts: Vec<String>,
    /// Batteries whose installed scripts this node will run on another node's
    /// orders.
    ///
    /// Declaring a battery is declaring its scripts, which is why the unit is
    /// the battery rather than each file: a battery is a versioned set with
    /// recorded provenance, so "everything from this source" is a statement
    /// someone can actually verify. Empty means none.
    ///
    /// Note what this does *not* grant: a remote peer still cannot install a
    /// battery. Installing remains a local act, so remote management can select
    /// among code the node already has and can never introduce more.
    #[serde(default)]
    pub remote_cue_batteries: Vec<String>,
    pub allow_baseline_push: bool,
    /// The baseline publishers this node will accept code from.
    ///
    /// The second of the two switches, on the pattern `remote_cue_scripts`
    /// established: `allow_baseline_push = true` opens the door, this says who
    /// may walk through it, and an empty list means nobody. Deny-by-default in
    /// the direction that matters — a node that turned the gate on and named no
    /// publisher installs nothing rather than trusting whoever signed first.
    ///
    /// A separate list from `authorities`, because they are separate keys with
    /// separate blast radii: an enrollment authority admits machines to the
    /// fleet, a publisher ships them code, and a node that recorded one in the
    /// other's slot would be granting a power nobody wrote down.
    #[serde(default)]
    pub baseline_publishers: Vec<TrustedBaselinePublisher>,
    #[serde(default)]
    pub authorities: Vec<EnrollmentAuthority>,
    #[serde(default)]
    pub bootstrap_token_hash: String,
    #[serde(default)]
    pub bootstrap_nonce_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentAuthority {
    pub key_id: String,
    pub public_key: String,
    #[serde(default)]
    pub revoked: bool,
}

/// A baseline publisher as a receiver records it.
///
/// Deliberately not `EnrollmentAuthority` despite the identical three fields.
/// The two lists authorize different things, and a shared type is how an entry
/// from one ends up satisfying a lookup in the other — a mistake no test would
/// see, because both would still be well-formed hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedBaselinePublisher {
    pub key_id: String,
    pub public_key: String,
    #[serde(default)]
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationSettings {
    pub id: String,
    pub discovery_secret_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscoverySettings {
    pub enabled: bool,
    pub port: u16,
    pub multicast_addr: String,
    pub broadcast: bool,
}

impl Default for DiscoverySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: crate::discovery::DISCOVERY_PORT,
            multicast_addr: crate::discovery::MULTICAST_GROUP.to_string(),
            broadcast: true,
        }
    }
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
                static_peers: Vec::new(),
                direct_bind: None,
            },
            trust: TrustSettings {
                enrollment: "disabled".to_string(),
                allow_remote_cues: false,
                remote_cue_scripts: Vec::new(),
                remote_cue_batteries: Vec::new(),
                allow_baseline_push: false,
                baseline_publishers: Vec::new(),
                authorities: Vec::new(),
                bootstrap_token_hash: String::new(),
                bootstrap_nonce_hash: String::new(),
            },
            discovery: DiscoverySettings::default(),
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
        self.validate_basic_fields()?;
        self.validate_network_settings()?;
        self.validate_enrollment_settings()?;
        self.validate_discovery_settings()?;
        self.validate_authorities()?;
        self.validate_baseline_publishers()?;
        self.validate_bootstrap_hashes()
    }

    fn validate_basic_fields(&self) -> Result<(), NodeConfigError> {
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

        Ok(())
    }

    fn validate_network_settings(&self) -> Result<(), NodeConfigError> {
        if self.network.static_peers.len() > MAX_NODE_CONFIG_STATIC_PEERS {
            return Err(NodeConfigError::Invalid(
                "network.static_peers has too many entries".to_string(),
            ));
        }
        for peer in &self.network.static_peers {
            validate_static_peer(peer)?;
        }
        let mut peer_ids = HashSet::new();
        let mut peer_endpoints = HashSet::new();
        for peer in &self.network.static_peers {
            let (node_id, endpoint) = peer.split_once('@').expect("validated static peer");
            if !peer_ids.insert(node_id) {
                return Err(NodeConfigError::Invalid(
                    "network.static_peers contains duplicate node ids".to_string(),
                ));
            }
            if !peer_endpoints.insert(endpoint) {
                return Err(NodeConfigError::Invalid(
                    "network.static_peers contains duplicate endpoints".to_string(),
                ));
            }
        }
        if let Some(bind) = &self.network.direct_bind {
            validate_text("network.direct_bind", bind, MAX_BIND_BYTES, false)?;
            validate_direct_bind(bind)?;
        }

        Ok(())
    }

    fn validate_enrollment_settings(&self) -> Result<(), NodeConfigError> {
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
                )));
            }
        }
        if self.trust.enrollment == "disabled"
            && (self.trust.allow_remote_cues || self.trust.allow_baseline_push)
        {
            return Err(NodeConfigError::Invalid(
                "remote capabilities require enrollment to be enabled".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_discovery_settings(&self) -> Result<(), NodeConfigError> {
        validate_secret_ref(&self.organization.discovery_secret_ref)?;
        if self.discovery.port != crate::discovery::DISCOVERY_PORT {
            return Err(NodeConfigError::Invalid(
                "discovery.port must use the frozen discovery port".to_string(),
            ));
        }
        let multicast = self
            .discovery
            .multicast_addr
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| {
                NodeConfigError::Invalid("discovery.multicast_addr is invalid".to_string())
            })?;
        if multicast != crate::discovery::MULTICAST_GROUP {
            return Err(NodeConfigError::Invalid(
                "discovery.multicast_addr must use the frozen discovery group".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_authorities(&self) -> Result<(), NodeConfigError> {
        if self.trust.authorities.len() > MAX_AUTHORITY_KEYS {
            return Err(NodeConfigError::Invalid(
                "trust.authorities has too many entries".to_string(),
            ));
        }
        let mut authority_ids = HashSet::new();
        for authority in &self.trust.authorities {
            validate_lower_hex("trust.authorities.key_id", &authority.key_id, 16)?;
            validate_lower_hex("trust.authorities.public_key", &authority.public_key, 32)?;
            if !authority_ids.insert(authority.key_id.as_str()) {
                return Err(NodeConfigError::Invalid(
                    "trust.authorities contains duplicate key IDs".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn validate_baseline_publishers(&self) -> Result<(), NodeConfigError> {
        if self.trust.baseline_publishers.len() > MAX_AUTHORITY_KEYS {
            return Err(NodeConfigError::Invalid(
                "trust.baseline_publishers has too many entries".to_string(),
            ));
        }
        let mut publisher_ids = HashSet::new();
        for publisher in &self.trust.baseline_publishers {
            validate_lower_hex("trust.baseline_publishers.key_id", &publisher.key_id, 16)?;
            validate_lower_hex(
                "trust.baseline_publishers.public_key",
                &publisher.public_key,
                32,
            )?;
            // A key id twice is a config whose meaning depends on which entry
            // the reader stops at, and one of the two could carry
            // `revoked = false`. Refused rather than resolved by order.
            if !publisher_ids.insert(publisher.key_id.as_str()) {
                return Err(NodeConfigError::Invalid(
                    "trust.baseline_publishers contains duplicate key IDs".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn validate_bootstrap_hashes(&self) -> Result<(), NodeConfigError> {
        if self.trust.enrollment == "signed-bundle" {
            if self.trust.authorities.is_empty() {
                return Err(NodeConfigError::Invalid(
                    "signed-bundle enrollment requires an authority".to_string(),
                ));
            }
            validate_lower_hex(
                "trust.bootstrap_token_hash",
                &self.trust.bootstrap_token_hash,
                32,
            )?;
            validate_lower_hex(
                "trust.bootstrap_nonce_hash",
                &self.trust.bootstrap_nonce_hash,
                32,
            )?;
        } else if !self.trust.bootstrap_token_hash.is_empty()
            || !self.trust.bootstrap_nonce_hash.is_empty()
        {
            return Err(NodeConfigError::Invalid(
                "bootstrap hashes require signed-bundle enrollment".to_string(),
            ));
        }
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

fn validate_lower_hex(field: &str, value: &str, bytes: usize) -> Result<(), NodeConfigError> {
    if value.len() != bytes * 2 || !hex::is_lower(value) {
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

fn validate_direct_bind(value: &str) -> Result<(), NodeConfigError> {
    let address = SocketAddr::from_str(value).map_err(|_| {
        NodeConfigError::Invalid("network.direct_bind must be a socket address".to_string())
    })?;
    if address.port() == 0 {
        return Err(NodeConfigError::Invalid(
            "network.direct_bind must use a non-zero port".to_string(),
        ));
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
    validate_host_port(endpoint).map_err(|reason| {
        NodeConfigError::Invalid(format!("static peer `{value}` is invalid: {reason}"))
    })
}

fn validate_host_port(value: &str) -> Result<(), &'static str> {
    let (host, port) = if value.starts_with('[') {
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
            base.replace("[network]", "[network]\nunsupported = true"),
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

    /// The list that says who may put code on this node has to be as strict as
    /// the one that says who may admit machines to the fleet.
    #[test]
    fn validation_rejects_malformed_and_duplicated_baseline_publishers() {
        let entry = |key_id: &str| TrustedBaselinePublisher {
            key_id: key_id.to_string(),
            public_key: "b".repeat(64),
            revoked: false,
        };

        let mut config = NodeConfig::default();
        config.trust.baseline_publishers = vec![entry(&"a".repeat(32))];
        config
            .validate()
            .expect("a well-formed publisher must be accepted");

        config.trust.baseline_publishers = vec![entry(&"a".repeat(31))];
        assert!(
            config.validate().is_err(),
            "a key id of the wrong length must not be accepted"
        );

        config.trust.baseline_publishers = vec![TrustedBaselinePublisher {
            public_key: "b".repeat(63),
            ..entry(&"a".repeat(32))
        }];
        assert!(
            config.validate().is_err(),
            "a public key of the wrong length must not be accepted"
        );

        config.trust.baseline_publishers = vec![entry(&"A".repeat(32))];
        assert!(
            config.validate().is_err(),
            "upper-case hex would make two spellings of one publisher"
        );

        // The duplicate matters more than it looks: the two entries can carry
        // different `revoked` values, and then what this node trusts depends on
        // which one a reader stops at.
        config.trust.baseline_publishers = vec![
            entry(&"a".repeat(32)),
            TrustedBaselinePublisher {
                revoked: true,
                ..entry(&"a".repeat(32))
            },
        ];
        assert!(
            config.validate().is_err(),
            "one publisher recorded twice must be refused, not resolved by order"
        );
    }

    /// The gate ships off, and the shipped default must stay that way.
    #[test]
    fn baseline_push_is_off_and_names_nobody_by_default() {
        let config = NodeConfig::default();
        assert!(!config.trust.allow_baseline_push);
        assert!(config.trust.baseline_publishers.is_empty());
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

        config = NodeConfig::default();
        config.discovery.port = 0;
        assert!(config.validate().is_err());
        config.discovery.port = 38383;
        config.discovery.multicast_addr = "127.0.0.1".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validation_accepts_canonical_peer_and_secret_ref() {
        let mut config = NodeConfig::default();
        config.network.static_peers = vec![format!("omk1_{}@127.0.0.1:7879", "a".repeat(64))];
        config.organization.discovery_secret_ref = "secret://prod/discovery_key".into();
        config.trust.enrollment = "manual".into();
        config.validate().unwrap();
    }

    #[test]
    fn validation_rejects_duplicate_static_peer_ids_and_endpoints() {
        let mut config = NodeConfig::default();
        let first_id = "a".repeat(64);
        let second_id = "b".repeat(64);
        config.network.static_peers = vec![
            format!("omk1_{first_id}@127.0.0.1:7879"),
            format!("omk1_{first_id}@127.0.0.1:7880"),
        ];
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("node ids")
        );

        config.network.static_peers = vec![
            format!("omk1_{first_id}@127.0.0.1:7879"),
            format!("omk1_{second_id}@127.0.0.1:7879"),
        ];
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("endpoints")
        );
    }

    #[test]
    fn validation_checks_all_peer_syntax_before_duplicates_and_direct_bind() {
        let first_id = "a".repeat(64);
        let mut config = NodeConfig::default();
        config.network.static_peers = vec![
            format!("omk1_{first_id}@127.0.0.1:7879"),
            format!("omk1_{first_id}@127.0.0.1:7880"),
            "invalid-id@127.0.0.1:7881".to_string(),
        ];
        config.network.direct_bind = Some("invalid-bind".to_string());
        assert_eq!(
            config.validate().unwrap_err(),
            NodeConfigError::Invalid(
                "static peer `invalid-id@127.0.0.1:7881` has an invalid node id".to_string()
            )
        );

        config.network.static_peers.pop();
        assert_eq!(
            config.validate().unwrap_err(),
            NodeConfigError::Invalid(
                "network.static_peers contains duplicate node ids".to_string()
            )
        );

        config.network.static_peers.pop();
        assert_eq!(
            config.validate().unwrap_err(),
            NodeConfigError::Invalid("network.direct_bind must be a socket address".to_string())
        );
    }

    #[test]
    fn validation_checks_discovery_and_keys_before_signed_bundle_hashes() {
        let mut config = NodeConfig::default();
        config.trust.enrollment = "signed-bundle".to_string();
        config.discovery.port = 0;
        config.trust.authorities = vec![EnrollmentAuthority {
            key_id: "invalid".to_string(),
            public_key: "b".repeat(64),
            revoked: false,
        }];
        config.trust.baseline_publishers = vec![TrustedBaselinePublisher {
            key_id: "invalid".to_string(),
            public_key: "b".repeat(64),
            revoked: false,
        }];
        assert_eq!(
            config.validate().unwrap_err(),
            NodeConfigError::Invalid(
                "discovery.port must use the frozen discovery port".to_string()
            )
        );

        config.discovery.port = crate::discovery::DISCOVERY_PORT;
        assert_eq!(
            config.validate().unwrap_err(),
            NodeConfigError::Invalid("trust.authorities.key_id is invalid".to_string())
        );

        config.trust.authorities[0].key_id = "a".repeat(32);
        assert_eq!(
            config.validate().unwrap_err(),
            NodeConfigError::Invalid("trust.baseline_publishers.key_id is invalid".to_string())
        );

        config.trust.baseline_publishers[0].key_id = "c".repeat(32);
        assert_eq!(
            config.validate().unwrap_err(),
            NodeConfigError::Invalid("trust.bootstrap_token_hash is invalid".to_string())
        );
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
        config.network.static_peers = vec![
            format!("omk1_{}@127.0.0.1:7879", "a".repeat(64));
            MAX_NODE_CONFIG_STATIC_PEERS + 1
        ];
        assert!(config.validate().is_err());

        config.network.static_peers.clear();
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
