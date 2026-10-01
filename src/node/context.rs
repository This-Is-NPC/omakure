use super::NodeError;
#[cfg(unix)]
use super::fs_unix::{owner_policy, validate_open_file_identity};
#[cfg(windows)]
use super::fs_windows::{validate_open_file_identity, validate_windows_security_handle};
use super::layout::{
    NODE_CONFIG_ENV, NODE_STATE_DIR_ENV, NODE_TEST_MODE_ENV, NodeLayout, NodePathOverrides,
    NodePlatform, default_layout, paths_overlap, validate_absolute_path,
};
#[cfg(not(unix))]
use super::security::owner_policy;
use super::security::{ensure_safe_parent_if_present, validate_file_security_metadata};
use crate::node_identity::NodeIdentityStatus;
use crate::node_registry::{NodeRegistry, RegistryError};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeContext {
    layout: NodeLayout,
    pub(super) test_mode: bool,
    pub(super) platform: NodePlatform,
    pub(super) custom_paths: bool,
}

impl NodeContext {
    /// Resolve node paths without touching the filesystem.
    pub fn resolve(overrides: NodePathOverrides) -> Result<Self, NodeError> {
        let test_mode = match env::var(NODE_TEST_MODE_ENV) {
            Ok(value) if value == "1" && cfg!(debug_assertions) => true,
            Ok(value) if value == "1" => return Err(NodeError::TestModeUnavailable),
            Ok(_) => return Err(NodeError::TestOverrideOutsideTestMode),
            Err(_) => false,
        };
        let env_state = env::var_os(NODE_STATE_DIR_ENV).map(PathBuf::from);
        let env_config = env::var_os(NODE_CONFIG_ENV).map(PathBuf::from);
        if (env_state.is_some() || env_config.is_some()) && !test_mode {
            return Err(NodeError::TestOverrideOutsideTestMode);
        }
        if test_mode && (env_state.is_some() != env_config.is_some()) {
            return Err(NodeError::IncompleteTestOverrides);
        }
        Self::resolve_for(
            NodePlatform::current(),
            overrides,
            test_mode,
            env_state,
            env_config,
            None,
        )
    }

    /// Resolve a platform layout from explicit inputs. This is kept separate
    /// from environment access so every platform mapping is deterministic in tests.
    pub fn resolve_for(
        platform: NodePlatform,
        cli_overrides: NodePathOverrides,
        test_mode: bool,
        env_state_dir: Option<PathBuf>,
        env_config_path: Option<PathBuf>,
        windows_program_data: Option<PathBuf>,
    ) -> Result<Self, NodeError> {
        let has_cli_overrides =
            cli_overrides.state_dir.is_some() || cli_overrides.config_path.is_some();
        if !test_mode && (has_cli_overrides || env_state_dir.is_some() || env_config_path.is_some())
        {
            return Err(NodeError::TestOverrideOutsideTestMode);
        }
        if test_mode && !cfg!(debug_assertions) {
            return Err(NodeError::TestModeUnavailable);
        }
        if test_mode && (env_state_dir.is_some() != env_config_path.is_some()) {
            return Err(NodeError::IncompleteTestOverrides);
        }
        let need_default_state = cli_overrides.state_dir.is_none() && env_state_dir.is_none();
        let need_default_config = cli_overrides.config_path.is_none() && env_config_path.is_none();
        let defaults = if need_default_state || need_default_config {
            Some(default_layout(platform, windows_program_data.as_deref())?)
        } else {
            None
        };
        let custom_paths = cli_overrides.state_dir.is_some()
            || cli_overrides.config_path.is_some()
            || env_state_dir.is_some()
            || env_config_path.is_some();
        let state_dir = cli_overrides
            .state_dir
            .or(env_state_dir)
            .unwrap_or_else(|| {
                defaults
                    .as_ref()
                    .expect("state default was requested")
                    .state_dir
                    .clone()
            });
        let config_path = cli_overrides
            .config_path
            .or(env_config_path)
            .unwrap_or_else(|| {
                defaults
                    .as_ref()
                    .expect("config default was requested")
                    .config_path
                    .clone()
            });
        validate_absolute_path(platform, "state directory", &state_dir, false)?;
        validate_absolute_path(platform, "config path", &config_path, true)?;
        if paths_overlap(&state_dir, &config_path) {
            return Err(NodeError::InvalidPath {
                field: "node paths",
                reason: "state directory and config path overlap".to_string(),
            });
        }
        Ok(Self {
            layout: NodeLayout {
                config_path,
                state_dir,
            },
            test_mode,
            platform,
            custom_paths,
        })
    }

    pub fn layout(&self) -> &NodeLayout {
        &self.layout
    }

    pub fn config_path(&self) -> &Path {
        self.layout.config_path()
    }

    pub fn state_dir(&self) -> &Path {
        self.layout.state_dir()
    }

    pub fn authority_key_path(&self) -> PathBuf {
        self.layout.authority_key_path()
    }

    pub fn publisher_key_path(&self) -> PathBuf {
        self.layout.publisher_key_path()
    }

    pub fn identity_path(&self) -> PathBuf {
        self.layout.identity_path()
    }

    pub fn database_path(&self) -> PathBuf {
        self.layout.database_path()
    }

    pub fn transport_key_path(&self) -> PathBuf {
        self.layout.transport_key_path()
    }

    pub fn transport_certificate_path(&self) -> PathBuf {
        self.layout.transport_certificate_path()
    }

    pub(crate) fn open_trust_registry(
        &self,
        identity: &NodeIdentityStatus,
    ) -> Result<NodeRegistry, RegistryError> {
        NodeRegistry::open(self, identity)
    }

    pub(crate) fn open_trust_registry_for_initialization(
        &self,
        identity: &NodeIdentityStatus,
    ) -> Result<NodeRegistry, RegistryError> {
        NodeRegistry::open_for_initialization(self, identity)
    }
}

impl NodeContext {
    /// Open the public configuration without following a final symlink or a
    /// reparse point, then validate the opened file's security metadata.
    pub(crate) fn open_public_file(&self) -> Result<Option<fs::File>, NodeError> {
        let path = self.config_path();
        if !ensure_safe_parent_if_present(path, self.test_mode)? {
            return Ok(None);
        }
        let mut options = crate::util::fs::no_follow_open_options();
        options.read(true);
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                return Err(NodeError::InsecurePath(format!(
                    "{} could not be opened securely",
                    path.display()
                )));
            }
        };
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(NodeError::UnexpectedFileType(
                "node configuration".to_string(),
            ));
        }
        validate_file_security_metadata(
            path,
            &metadata,
            owner_policy(self.platform, self.custom_paths, false)?,
            self.test_mode,
            0o640,
        )?;
        if !ensure_safe_parent_if_present(path, self.test_mode)? {
            return Err(NodeError::InsecurePath(format!(
                "{} changed while it was being opened",
                path.display()
            )));
        }
        validate_open_file_identity(path, &file)?;
        #[cfg(windows)]
        {
            validate_windows_security_handle(path, &file, self.test_mode)?;
            // Recheck after handle-bound ACL validation as a final defense
            // against path replacement during the security decision.
            validate_open_file_identity(path, &file)?;
        }
        Ok(Some(file))
    }
}
