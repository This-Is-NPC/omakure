use super::NodeError;
use std::env;
use std::path::{Component, Path, PathBuf};

pub(super) const NODE_TEST_MODE_ENV: &str = "OMAKURE_NODE_TEST_MODE";

pub(super) const NODE_STATE_DIR_ENV: &str = "OMAKURE_NODE_STATE_DIR";

pub(super) const NODE_CONFIG_ENV: &str = "OMAKURE_NODE_CONFIG";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodePlatform {
    Linux,
    MacOs,
    Windows,
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("node layout is supported only on Linux, macOS, and Windows");

impl NodePlatform {
    #[cfg(target_os = "linux")]
    pub fn current() -> Self {
        Self::Linux
    }

    #[cfg(target_os = "macos")]
    pub fn current() -> Self {
        Self::MacOs
    }

    #[cfg(target_os = "windows")]
    pub fn current() -> Self {
        Self::Windows
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodePathOverrides {
    pub state_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
}

impl NodePathOverrides {
    pub fn new(state_dir: Option<PathBuf>, config_path: Option<PathBuf>) -> Self {
        Self {
            state_dir,
            config_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLayout {
    pub(super) config_path: PathBuf,
    pub(super) state_dir: PathBuf,
}

impl NodeLayout {
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Where the enrollment-authority signing key lives, when this node holds
    /// one. Beside the identity, under the same 0700 directory and the same
    /// 0600 discipline, but a *separate* key: reusing the identity would mean
    /// compromising one node hands over the right to enrol the whole fleet.
    pub fn authority_key_path(&self) -> PathBuf {
        self.state_dir.join(AUTHORITY_KEY_FILE)
    }

    /// Where the baseline-publisher signing key lives, when this node holds
    /// one. A third key beside the identity and the authority, for the same
    /// reason there is a second: the key that admits a machine to the fleet and
    /// the key that ships that machine code have different blast radii, and
    /// folding them together would choose the larger one for everybody.
    pub fn publisher_key_path(&self) -> PathBuf {
        self.state_dir.join(PUBLISHER_KEY_FILE)
    }

    pub fn identity_path(&self) -> PathBuf {
        self.state_dir.join(IDENTITY_KEY_FILE)
    }

    pub fn database_path(&self) -> PathBuf {
        self.state_dir.join(DATABASE_FILE)
    }

    pub fn transport_key_path(&self) -> PathBuf {
        self.state_dir.join(TRANSPORT_KEY_FILE)
    }

    pub fn transport_certificate_path(&self) -> PathBuf {
        self.state_dir.join(TRANSPORT_CERTIFICATE_FILE)
    }
}

/// Files a node keeps in its private state directory. The allow-list in
/// `validate_existing_state_contents` admits exactly these, plus the SQLite
/// write-ahead companions of [`DATABASE_FILE`].
pub const IDENTITY_KEY_FILE: &str = "identity.key";

pub const AUTHORITY_KEY_FILE: &str = "authority.key";

pub const PUBLISHER_KEY_FILE: &str = "publisher.key";

pub const DATABASE_FILE: &str = "node.sqlite";

pub const TRANSPORT_KEY_FILE: &str = "transport.key";

pub const TRANSPORT_CERTIFICATE_FILE: &str = "transport.cert";

pub const IDENTITY_LOCK_FILE: &str = ".identity.lock";

pub const LIFECYCLE_LOCK_FILE: &str = ".node.lifecycle.lock";

/// A public-key file beside the identity, which the identity state refuses.
pub const IDENTITY_PUBLIC_FILE: &str = "identity.pub";

/// Why an operation that needs existing node state refused to create it.
pub const STATE_NOT_INITIALIZED: &str = "node state is not initialized";

pub fn default_layout(
    platform: NodePlatform,
    windows_program_data: Option<&Path>,
) -> Result<NodeLayout, NodeError> {
    let (config_path, state_dir) = match platform {
        NodePlatform::Linux => (
            PathBuf::from("/etc/omakure/node.toml"),
            PathBuf::from("/var/lib/omakure"),
        ),
        NodePlatform::MacOs => (
            PathBuf::from("/Library/Application Support/Omakure/node.toml"),
            PathBuf::from("/Library/Application Support/Omakure"),
        ),
        NodePlatform::Windows => {
            let root = windows_program_data
                .map(Path::to_path_buf)
                .or_else(|| env::var_os("ProgramData").map(PathBuf::from))
                .ok_or_else(|| NodeError::InvalidPath {
                    field: "ProgramData",
                    reason: "ProgramData is not set".to_string(),
                })?;
            (root.join("Omakure/node.toml"), root.join("Omakure"))
        }
    };
    Ok(NodeLayout {
        config_path,
        state_dir,
    })
}

pub(super) fn validate_absolute_path(field: &'static str, path: &Path) -> Result<(), NodeError> {
    if !path.is_absolute() {
        return Err(NodeError::InvalidPath {
            field,
            reason: "path must be absolute".to_string(),
        });
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(NodeError::InvalidPath {
            field,
            reason: "path contains an unsafe component".to_string(),
        });
    }
    if !path
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
    {
        return Err(NodeError::InvalidPath {
            field,
            reason: "path is not a usable node path".to_string(),
        });
    }
    Ok(())
}

pub(super) fn paths_overlap(state_dir: &Path, config_path: &Path) -> bool {
    let shared_config = config_path == state_dir.join("node.toml");
    !shared_config
        && (state_dir == config_path
            || config_path.starts_with(state_dir)
            || state_dir.starts_with(config_path))
}
