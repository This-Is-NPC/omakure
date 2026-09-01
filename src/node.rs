use crate::domain::{NodeConfig, NodeConfigError};
use crate::node_identity::NodeIdentityStatus;
use crate::node_registry::{NodeRegistry, RegistryError};
use fs2::FileExt;
use rand::rngs::OsRng;
use rand::RngCore;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
#[cfg(not(windows))]
use std::io::{Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[cfg(test)]
use std::sync::atomic::{AtomicU8, Ordering};

const NODE_TEST_MODE_ENV: &str = "OMAKURE_NODE_TEST_MODE";
const NODE_STATE_DIR_ENV: &str = "OMAKURE_NODE_STATE_DIR";
const NODE_CONFIG_ENV: &str = "OMAKURE_NODE_CONFIG";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodePlatform {
    Linux,
    MacOs,
    Windows,
}

impl NodePlatform {
    pub fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            return Self::Linux;
        }
        #[cfg(target_os = "macos")]
        {
            return Self::MacOs;
        }
        #[cfg(target_os = "windows")]
        {
            return Self::Windows;
        }
        #[allow(unreachable_code)]
        Self::Linux
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
    config_path: PathBuf,
    state_dir: PathBuf,
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
        self.state_dir.join("authority.key")
    }

    /// Where the baseline-publisher signing key lives, when this node holds
    /// one. A third key beside the identity and the authority, for the same
    /// reason there is a second: the key that admits a machine to the fleet and
    /// the key that ships that machine code have different blast radii, and
    /// folding them together would choose the larger one for everybody.
    pub fn publisher_key_path(&self) -> PathBuf {
        self.state_dir.join("publisher.key")
    }

    pub fn identity_path(&self) -> PathBuf {
        self.state_dir.join("identity.key")
    }

    pub fn database_path(&self) -> PathBuf {
        self.state_dir.join("node.sqlite")
    }

    pub fn transport_key_path(&self) -> PathBuf {
        self.state_dir.join("transport.key")
    }

    pub fn transport_certificate_path(&self) -> PathBuf {
        self.state_dir.join("transport.cert")
    }
}

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("invalid node path for {field}: {reason}")]
    InvalidPath { field: &'static str, reason: String },
    #[error("node test-mode overrides require {NODE_STATE_DIR_ENV} and {NODE_CONFIG_ENV}")]
    IncompleteTestOverrides,
    #[error(
        "{NODE_STATE_DIR_ENV} and {NODE_CONFIG_ENV} are only allowed with {NODE_TEST_MODE_ENV}=1"
    )]
    TestOverrideOutsideTestMode,
    #[error("node test mode is unavailable in this build")]
    TestModeUnavailable,
    #[error("node path is unsafe: {0}")]
    UnsafePath(String),
    #[error("node path has unexpected file type: {0}")]
    UnexpectedFileType(String),
    #[error("node path is insecure: {0}")]
    InsecurePath(String),
    #[error("node configuration already exists and is invalid: {0}")]
    ExistingConfig(String),
    #[error("node service lifecycle lock is busy")]
    LifecycleBusy,
    #[error("node configuration error: {0}")]
    Config(#[from] NodeConfigError),
    #[error("node I/O error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInitialization {
    pub state_dir_created: bool,
    pub config_created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeContext {
    layout: NodeLayout,
    test_mode: bool,
    platform: NodePlatform,
    custom_paths: bool,
}

const PRIVATE_TOKEN_TOMBSTONE_PREFIX: &str = ".omakure-bootstrap-token-";
pub(crate) const PRIVATE_TOKEN_TOMBSTONE_RETRY_LIMIT: usize = 10;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateTokenFault {
    None = 0,
    Rename = 1,
    Restore = 2,
    Delete = 3,
}

#[cfg(test)]
static PRIVATE_TOKEN_FAULT: AtomicU8 = AtomicU8::new(PrivateTokenFault::None as u8);

#[cfg(test)]
pub(crate) fn set_private_token_fault(fault: PrivateTokenFault) {
    PRIVATE_TOKEN_FAULT.store(fault as u8, Ordering::SeqCst);
}

#[cfg(test)]
fn private_token_fault(fault: PrivateTokenFault) -> bool {
    PRIVATE_TOKEN_FAULT.load(Ordering::SeqCst) == fault as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateFileCommitStatus {
    Clean,
    CleanupRequired,
}

pub(crate) struct PrivateTokenLease {
    original_path: PathBuf,
    tombstone_path: PathBuf,
    file: fs::File,
    contents: Vec<u8>,
    _lock: Option<PrivateTokenLock>,
}

struct OpenedPrivateFile {
    file: fs::File,
    contents: Vec<u8>,
}

pub(crate) struct PrivateTokenLock {
    _file: fs::File,
}

impl PrivateTokenLease {
    pub(crate) fn contents(&self) -> &[u8] {
        &self.contents
    }

    pub(crate) fn restore(self) -> Result<(), NodeError> {
        #[cfg(test)]
        if private_token_fault(PrivateTokenFault::Restore) {
            return Err(NodeError::Io(io::Error::other(
                "injected bootstrap token restore failure",
            )));
        }
        if fs::symlink_metadata(&self.original_path).is_ok() {
            return Err(NodeError::InsecurePath(
                "bootstrap token path was recreated during enrollment".to_string(),
            ));
        }
        fs::rename(&self.tombstone_path, &self.original_path)?;
        sync_directory(self.original_path.parent().ok_or_else(|| {
            NodeError::UnsafePath("bootstrap token parent is missing".to_string())
        })?)?;
        Ok(())
    }

    pub(crate) fn finish_success(mut self) -> PrivateFileCommitStatus {
        let mut cleanup_required = false;
        #[cfg(not(windows))]
        if self.file.seek(SeekFrom::Start(0)).is_err()
            || self
                .file
                .write_all(&vec![0_u8; self.contents.len()])
                .is_err()
            || self.file.set_len(0).is_err()
            || self.file.sync_all().is_err()
        {
            cleanup_required = true;
        }
        #[cfg(test)]
        let delete_failed = private_token_fault(PrivateTokenFault::Delete);
        #[cfg(not(test))]
        let delete_failed = false;
        drop(self.file);
        let deleted = !delete_failed && fs::remove_file(&self.tombstone_path).is_ok();
        let directory_sync_failed = deleted
            && self
                .tombstone_path
                .parent()
                .map(sync_directory)
                .transpose()
                .is_err();
        cleanup_required |= !deleted || directory_sync_failed;
        if cleanup_required {
            PrivateFileCommitStatus::CleanupRequired
        } else {
            PrivateFileCommitStatus::Clean
        }
    }
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

    pub fn is_test_mode(&self) -> bool {
        self.test_mode
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

    /// Create only the state directory and public config. Identity and the
    /// trust database are deliberately not created by this foundation layer.
    pub fn initialize(&self, config: &NodeConfig) -> Result<NodeInitialization, NodeError> {
        config.validate()?;
        let config_parent = self
            .config_path()
            .parent()
            .ok_or_else(|| NodeError::InvalidPath {
                field: "config path",
                reason: "config path has no parent".to_string(),
            })?;
        let shared_config = config_parent == self.state_dir();
        if !shared_config {
            ensure_safe_parent(config_parent)?;
        }

        let state_dir_created = self.ensure_state_directory()?;

        if shared_config {
            ensure_safe_parent(config_parent)?;
        }
        let config_preexisting = !matches!(
            fs::symlink_metadata(self.config_path()),
            Err(err) if err.kind() == io::ErrorKind::NotFound
        );
        let config_result: Result<bool, NodeError> =
            (|| match fs::symlink_metadata(self.config_path()) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                        return Err(NodeError::UnexpectedFileType(
                            self.config_path().display().to_string(),
                        ));
                    }
                    validate_file_security(
                        self.config_path(),
                        owner_policy(self.platform, self.custom_paths, false)?,
                        self.test_mode,
                    )?;
                    let contents = fs::read_to_string(self.config_path())?;
                    NodeConfig::parse(&contents)
                        .map_err(|err| NodeError::ExistingConfig(err.to_string()))?;
                    Ok(false)
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    let contents = config.to_toml()?;
                    let created = match write_atomic_config(self.config_path(), contents.as_bytes())
                    {
                        Ok(()) => true,
                        // Another first start may win the config race. Never
                        // replace its file; validate and converge on it.
                        Err(NodeError::Io(error))
                            if error.kind() == io::ErrorKind::AlreadyExists =>
                        {
                            let metadata = fs::symlink_metadata(self.config_path())?;
                            if metadata.file_type().is_symlink() || !metadata.file_type().is_file()
                            {
                                return Err(NodeError::UnexpectedFileType(
                                    self.config_path().display().to_string(),
                                ));
                            }
                            validate_file_security(
                                self.config_path(),
                                owner_policy(self.platform, self.custom_paths, false)?,
                                self.test_mode,
                            )?;
                            let existing = fs::read_to_string(self.config_path())?;
                            NodeConfig::parse(&existing)
                                .map_err(|err| NodeError::ExistingConfig(err.to_string()))?;
                            false
                        }
                        Err(error) => return Err(error),
                    };
                    validate_file_security(
                        self.config_path(),
                        owner_policy(self.platform, self.custom_paths, false)?,
                        self.test_mode,
                    )?;
                    Ok(created)
                }
                Err(err) => Err(err.into()),
            })();

        let config_created = match config_result {
            Ok(created) => created,
            Err(err) => {
                if state_dir_created {
                    cleanup_partial_initialization(
                        self.state_dir(),
                        self.config_path(),
                        !config_preexisting,
                    )?;
                }
                return Err(err);
            }
        };

        Ok(NodeInitialization {
            state_dir_created,
            config_created,
        })
    }

    /// Ensure the machine-owned state directory exists and is secure.
    pub(crate) fn ensure_state_directory(&self) -> Result<bool, NodeError> {
        ensure_safe_parent(self.state_dir())?;
        let created = match fs::symlink_metadata(self.state_dir()) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                    return Err(NodeError::UnexpectedFileType(
                        self.state_dir().display().to_string(),
                    ));
                }
                false
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                match create_secure_directory(self.state_dir()) {
                    Ok(()) => true,
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => false,
                    Err(err) => return Err(err.into()),
                }
            }
            Err(err) => return Err(err.into()),
        };
        if created {
            if let Err(err) = (|| {
                set_directory_mode(self.state_dir())?;
                validate_directory_security(
                    self.state_dir(),
                    owner_policy(self.platform, self.custom_paths, true)?,
                    self.test_mode,
                )
            })() {
                let _ = fs::remove_dir(self.state_dir());
                return Err(err);
            }
        } else {
            validate_directory_security(
                self.state_dir(),
                owner_policy(self.platform, self.custom_paths, true)?,
                self.test_mode,
            )?;
        }
        Ok(created)
    }

    /// Validate an already-created state directory without creating anything.
    /// Status and other read-only management operations use this boundary so
    /// observation cannot initialize node state as a side effect.
    pub(crate) fn validate_existing_state_directory(&self) -> Result<bool, NodeError> {
        match fs::symlink_metadata(self.state_dir()) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                    return Err(NodeError::UnexpectedFileType(
                        self.state_dir().display().to_string(),
                    ));
                }
                validate_directory_security(
                    self.state_dir(),
                    owner_policy(self.platform, self.custom_paths, true)?,
                    self.test_mode,
                )?;
                Ok(true)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Reject state entries that are not owned by the node persistence
    /// contract. This is intentionally observational: it never repairs or
    /// removes an entry.
    pub(crate) fn validate_existing_state_contents(&self) -> Result<bool, NodeError> {
        if !self.validate_existing_state_directory()? {
            return Ok(false);
        }
        for entry in fs::read_dir(self.state_dir())? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(NodeError::UnexpectedFileType(name.into_owned()));
            }
            let allowed = matches!(
                name.as_ref(),
                "identity.key"
                    // The enrollment-authority signing key, on a node that
                    // issues fleet membership. Added to this closed list
                    // deliberately: the list is a security control, and a new
                    // entry is an amendment to it, not a convenience.
                    | "authority.key"
                    // The baseline-publisher signing key, on a node that ships
                    // code to the fleet. Second amendment to this closed list,
                    // held to the same standard as the first: the list is the
                    // control, and every entry is a decision to admit one more
                    // file to the node's private state.
                    | "publisher.key"
                    | "node.sqlite"
                    | "node.sqlite-wal"
                    | "node.sqlite-shm"
                    | "transport.key"
                    | "transport.cert"
                    | ".identity.lock"
                    | ".node.lifecycle.lock"
                    | "node.toml"
            ) || name
                .strip_prefix(".cue-execution-")
                .and_then(|digest| digest.strip_suffix(".lock"))
                .is_some_and(|digest| {
                    digest.len() == 64
                        && digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                });
            if !allowed {
                return Err(NodeError::InsecurePath(format!(
                    "unsupported node state entry {name:?}"
                )));
            }
        }
        Ok(true)
    }

    pub(crate) fn acquire_lifecycle_lock(&self) -> Result<NodeLifecycleLock, NodeError> {
        NodeLifecycleLock::acquire(self, false)
    }

    pub(crate) fn try_acquire_lifecycle_lock(&self) -> Result<NodeLifecycleLock, NodeError> {
        NodeLifecycleLock::acquire(self, true)
    }

    pub(crate) fn validate_private_file(&self, path: &Path) -> Result<(), NodeError> {
        validate_file_security_mode(
            path,
            owner_policy(self.platform, self.custom_paths, true)?,
            self.test_mode,
            0o600,
        )
    }

    /// Read a node-local private file without following path reparse points or
    /// allowing an unbounded allocation. The service owner and exact 0600
    /// permissions are part of the file contract.
    pub(crate) fn stage_private_bounded_file(
        &self,
        path: &Path,
        max_bytes: usize,
    ) -> Result<PrivateTokenLease, NodeError> {
        let lock = self.acquire_private_token_lock(path)?;
        let file = self.open_private_bounded_file(path, max_bytes)?;
        let parent = path
            .parent()
            .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
        let tombstone_path = private_token_tombstone_path(path)?;
        if fs::symlink_metadata(&tombstone_path).is_ok() {
            return Err(NodeError::InsecurePath(
                "bootstrap token cleanup is pending".to_string(),
            ));
        }
        #[cfg(test)]
        if private_token_fault(PrivateTokenFault::Rename) {
            return Err(NodeError::Io(io::Error::other(
                "injected bootstrap token rename failure",
            )));
        }
        fs::rename(path, &tombstone_path)?;
        if let Err(error) = sync_directory(parent) {
            let _ = fs::rename(&tombstone_path, path);
            let _ = sync_directory(parent);
            return Err(NodeError::Io(error));
        }
        if let Err(error) = validate_open_file_identity(&tombstone_path, &file.file) {
            let _ = fs::rename(&tombstone_path, path);
            let _ = sync_directory(parent);
            return Err(error);
        }
        Ok(PrivateTokenLease {
            original_path: path.to_path_buf(),
            tombstone_path,
            file: file.file,
            contents: file.contents,
            _lock: Some(lock),
        })
    }

    pub(crate) fn acquire_private_token_lock(
        &self,
        path: &Path,
    ) -> Result<PrivateTokenLock, NodeError> {
        validate_absolute_path(self.platform, "private file", path, true)?;
        ensure_safe_parent(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
        let lock_path = parent.join(".omakure-bootstrap-token.lock");
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&lock_path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(NodeError::UnexpectedFileType(
                "bootstrap token lock".to_string(),
            ));
        }
        validate_file_security_metadata(
            &lock_path,
            &metadata,
            owner_policy(self.platform, self.custom_paths, true)?,
            self.test_mode,
            0o600,
        )?;
        file.lock_exclusive()?;
        Ok(PrivateTokenLock { _file: file })
    }

    pub(crate) fn list_private_token_tombstones(
        &self,
        path: &Path,
        max_bytes: usize,
    ) -> Result<Vec<PrivateTokenLease>, NodeError> {
        validate_absolute_path(self.platform, "private file", path, true)?;
        ensure_safe_parent(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?
            .to_string_lossy();
        let suffix = format!("-{file_name}");
        let mut tombstones = Vec::new();
        for entry in fs::read_dir(parent)?.take(PRIVATE_TOKEN_TOMBSTONE_RETRY_LIMIT) {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(PRIVATE_TOKEN_TOMBSTONE_PREFIX) || !name.ends_with(&suffix) {
                continue;
            }
            let tombstone = entry.path();
            let file = self.open_private_bounded_file(&tombstone, max_bytes)?;
            tombstones.push(PrivateTokenLease {
                original_path: path.to_path_buf(),
                tombstone_path: tombstone,
                file: file.file,
                contents: file.contents,
                _lock: None,
            });
        }
        Ok(tombstones)
    }

    fn open_private_bounded_file(
        &self,
        path: &Path,
        max_bytes: usize,
    ) -> Result<OpenedPrivateFile, NodeError> {
        validate_absolute_path(self.platform, "private file", path, true)?;
        ensure_safe_parent(path)?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(not(windows))]
        options.write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(NodeError::UnexpectedFileType(path.display().to_string()));
        }
        validate_file_security_metadata(
            path,
            &metadata,
            owner_policy(self.platform, self.custom_paths, true)?,
            self.test_mode,
            0o600,
        )?;
        validate_open_file_identity(path, &file)?;
        #[cfg(windows)]
        validate_windows_security_handle(path, &file, self.test_mode)?;
        if metadata.len() > max_bytes as u64 {
            return Err(NodeError::InsecurePath(format!(
                "private file exceeds the {max_bytes}-byte bound"
            )));
        }
        let mut limited = (&file).take(max_bytes as u64 + 1);
        let mut contents = Vec::with_capacity(metadata.len() as usize);
        limited.read_to_end(&mut contents)?;
        if contents.len() > max_bytes {
            return Err(NodeError::InsecurePath(format!(
                "private file exceeds the {max_bytes}-byte bound"
            )));
        }
        Ok(OpenedPrivateFile { file, contents })
    }

    /// Open the public configuration without following a final symlink or a
    /// reparse point, then validate the opened file's security metadata.
    pub(crate) fn open_public_file(&self) -> Result<Option<fs::File>, NodeError> {
        let path = self.config_path();
        if !ensure_safe_parent_if_present(path)? {
            return Ok(None);
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                return Err(NodeError::InsecurePath(format!(
                    "{} could not be opened securely",
                    path.display()
                )))
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
        if !ensure_safe_parent_if_present(path)? {
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

/// Serializes the complete machine-node lifecycle, not just identity writes.
/// The file remains after reset so Windows never needs to unlink an open lock
/// or race a new service between deletion and cleanup.
pub(crate) struct NodeLifecycleLock {
    file: fs::File,
    state_was_present: bool,
}

impl NodeLifecycleLock {
    fn acquire(context: &NodeContext, nonblocking: bool) -> Result<Self, NodeError> {
        let state_was_present = prepare_lifecycle_state(context)?;
        let path = context.state_dir().join(".node.lifecycle.lock");
        let file = open_lifecycle_lock(context, &path, nonblocking)?;
        let file = lock_lifecycle_file(file, nonblocking)?;
        Ok(Self {
            file,
            state_was_present,
        })
    }

    pub(crate) fn state_was_present(&self) -> bool {
        self.state_was_present
    }
}

fn prepare_lifecycle_state(context: &NodeContext) -> Result<bool, NodeError> {
    let state_was_present = match fs::symlink_metadata(context.state_dir()) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    context.ensure_state_directory()?;
    Ok(state_was_present)
}

fn open_lifecycle_lock(
    context: &NodeContext,
    path: &Path,
    nonblocking: bool,
) -> Result<fs::File, NodeError> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(path)
        .map_err(|error| classify_lifecycle_lock_error(error, nonblocking))?;
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(NodeError::InsecurePath(
            "node lifecycle lock is a symlink".to_string(),
        ));
    }
    context.validate_private_file(path)?;
    Ok(file)
}

fn lock_lifecycle_file(file: fs::File, nonblocking: bool) -> Result<fs::File, NodeError> {
    let result = if nonblocking {
        file.try_lock_exclusive()
    } else {
        file.lock_exclusive()
    };
    result
        .map(|()| file)
        .map_err(|error| classify_lifecycle_lock_error(error, nonblocking))
}

fn classify_lifecycle_lock_error(error: io::Error, nonblocking: bool) -> NodeError {
    if nonblocking && is_lifecycle_lock_contention(&error) {
        NodeError::LifecycleBusy
    } else {
        error.into()
    }
}

fn is_lifecycle_lock_contention(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock || error.kind() == io::ErrorKind::PermissionDenied
    {
        return true;
    }
    #[cfg(windows)]
    {
        // Windows reports sharing and lock violations as raw Win32 errors
        // instead of mapping them to WouldBlock.
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

impl Drop for NodeLifecycleLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

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

fn validate_absolute_path(
    platform: NodePlatform,
    field: &'static str,
    path: &Path,
    is_file: bool,
) -> Result<(), NodeError> {
    if !path.is_absolute() {
        return Err(NodeError::InvalidPath {
            field,
            reason: "path must be absolute".to_string(),
        });
    }
    if path.components().any(|component| match component {
        Component::ParentDir | Component::CurDir => true,
        #[cfg(windows)]
        Component::Prefix(_) => false,
        #[cfg(not(windows))]
        Component::Prefix(_) => platform != NodePlatform::Windows,
        _ => false,
    }) {
        return Err(NodeError::InvalidPath {
            field,
            reason: "path contains an unsafe component".to_string(),
        });
    }
    if path.parent().is_none()
        || !path
            .components()
            .any(|component| matches!(component, Component::Normal(_)))
    {
        return Err(NodeError::InvalidPath {
            field,
            reason: "path is not a usable node path".to_string(),
        });
    }
    if is_file && path.file_name().is_none() {
        return Err(NodeError::InvalidPath {
            field,
            reason: "config path must name a file".to_string(),
        });
    }
    Ok(())
}

fn paths_overlap(state_dir: &Path, config_path: &Path) -> bool {
    let shared_config = config_path == state_dir.join("node.toml");
    !shared_config
        && (state_dir == config_path
            || config_path.starts_with(state_dir)
            || state_dir.starts_with(config_path))
}

fn cleanup_partial_initialization(
    state_dir: &Path,
    config_path: &Path,
    remove_config: bool,
) -> Result<(), NodeError> {
    if remove_config {
        match fs::symlink_metadata(config_path) {
            Ok(_) => fs::remove_file(config_path)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    fs::remove_dir(state_dir)?;
    Ok(())
}

fn ensure_safe_parent(path: &Path) -> Result<(), NodeError> {
    if !ensure_safe_parent_if_present(path)? {
        return Err(NodeError::UnsafePath(format!(
            "parent does not exist: {}",
            path.parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_else(|| path.display().to_string())
        )));
    }
    Ok(())
}

fn ensure_safe_parent_if_present(path: &Path) -> Result<bool, NodeError> {
    let parent = path
        .parent()
        .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
    let mut current = PathBuf::new();
    for component in parent.components() {
        current.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(NodeError::Io(error)),
        };
        if metadata.file_type().is_symlink() {
            return Err(NodeError::UnsafePath(current.display().to_string()));
        }
        #[cfg(windows)]
        if windows_has_reparse_point(&current)? {
            return Err(NodeError::UnsafePath(current.display().to_string()));
        }
        if !metadata.file_type().is_dir() {
            return Err(NodeError::UnexpectedFileType(current.display().to_string()));
        }
    }
    Ok(true)
}

fn private_token_tombstone_path(path: &Path) -> Result<PathBuf, NodeError> {
    let parent = path
        .parent()
        .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?
        .to_string_lossy();
    let mut random = [0_u8; 16];
    OsRng.fill_bytes(&mut random);
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(parent.join(format!(
        "{PRIVATE_TOKEN_TOMBSTONE_PREFIX}{suffix}-{file_name}"
    )))
}

#[cfg(unix)]
fn validate_open_file_identity(path: &Path, opened_file: &fs::File) -> Result<(), NodeError> {
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.file_type().is_file() {
        return Err(NodeError::InsecurePath(
            "node configuration path is not a regular file".to_string(),
        ));
    }
    let opened_metadata = opened_file.metadata()?;
    if !same_file_identity(&opened_metadata, &path_metadata) {
        return Err(NodeError::InsecurePath(
            "node configuration path changed while opening".to_string(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowsFileIdentity {
    volume_serial_number: u32,
    file_index: u64,
    reparse_point: bool,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Debug, Default)]
struct ByHandleFileInformation {
    file_attributes: u32,
    creation_time: [u32; 2],
    last_access_time: [u32; 2],
    last_write_time: [u32; 2],
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[cfg(windows)]
fn validate_open_file_identity(path: &Path, opened_file: &fs::File) -> Result<(), NodeError> {
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.file_type().is_file() {
        return Err(NodeError::InsecurePath(
            "node configuration path is not a regular file".to_string(),
        ));
    }
    if windows_has_reparse_point(path)? {
        return Err(NodeError::UnsafePath(
            "node configuration path has a reparse point".to_string(),
        ));
    }
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let current_file = options.open(path).map_err(NodeError::Io)?;
    let opened_identity = windows_file_identity(opened_file)?;
    let current_identity = windows_file_identity(&current_file)?;
    if opened_identity.reparse_point || current_identity.reparse_point {
        return Err(NodeError::UnsafePath(
            "node configuration path has a reparse point".to_string(),
        ));
    }
    if opened_identity != current_identity {
        return Err(NodeError::InsecurePath(
            "node configuration path changed while opening".to_string(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_file_identity(file: &fs::File) -> Result<WindowsFileIdentity, NodeError> {
    use std::os::windows::io::AsRawHandle;

    let mut information = ByHandleFileInformation::default();
    let success = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
    if success == 0 {
        return Err(NodeError::Io(io::Error::last_os_error()));
    }
    Ok(WindowsFileIdentity {
        volume_serial_number: information.volume_serial_number,
        file_index: (u64::from(information.file_index_high) << 32)
            | u64::from(information.file_index_low),
        reparse_point: information.file_attributes & 0x400 != 0,
    })
}

fn write_atomic_config(path: &Path, contents: &[u8]) -> Result<(), NodeError> {
    write_atomic_new(path, contents, 0o640)
}

pub(crate) fn write_atomic_new(
    path: &Path,
    contents: &[u8],
    #[cfg(unix)] mode: u32,
    #[cfg(not(unix))] _mode: u32,
) -> Result<(), NodeError> {
    let parent = path
        .parent()
        .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
    let mut random = [0u8; 8];
    OsRng.fill_bytes(&mut random);
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let temp = parent.join(format!(
        ".{}.tmp-{}-{suffix}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        let mut file = options.open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        // Linking a staged file creates the destination only if it did not
        // appear concurrently; unlike rename, it never clobbers a config.
        fs::hard_link(&temp, path)?;
        fs::remove_file(&temp)?;
        sync_directory(parent)?;
        Ok::<(), io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(NodeError::Io)
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn set_directory_mode(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let _ = path;
    Ok(())
}

fn create_secure_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "node path contains a NUL byte")
        })?;
        let result = unsafe { libc::mkdir(path.as_ptr(), 0o700) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct UnixOwner {
    uid: u32,
    gid: u32,
}

#[cfg(unix)]
const PRINCIPAL_LOOKUP_BUFFER_LIMIT: usize = 1024 * 1024;

#[cfg(unix)]
fn grow_principal_lookup_buffer(buffer: &mut Vec<u8>) -> Result<(), NodeError> {
    if buffer.len() >= PRINCIPAL_LOOKUP_BUFFER_LIMIT {
        return Err(NodeError::InsecurePath(
            "configured node service principal lookup exceeded the supported size".to_string(),
        ));
    }
    buffer.resize(
        buffer
            .len()
            .saturating_mul(2)
            .min(PRINCIPAL_LOOKUP_BUFFER_LIMIT),
        0,
    );
    Ok(())
}

#[cfg(unix)]
fn lookup_unix_principal(
    user_name: &std::ffi::CStr,
    group_name: &std::ffi::CStr,
) -> Result<UnixOwner, NodeError> {
    use std::ptr;

    let uid = {
        let mut entry = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut result = ptr::null_mut();
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let status = unsafe {
                libc::getpwnam_r(
                    user_name.as_ptr(),
                    &mut entry,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            };
            if status == libc::ERANGE {
                grow_principal_lookup_buffer(&mut buffer)?;
                continue;
            }
            if status != 0 {
                return Err(NodeError::InsecurePath(format!(
                    "failed to resolve configured node service user: {}",
                    io::Error::from_raw_os_error(status)
                )));
            }
            if result.is_null() {
                return Err(NodeError::InsecurePath(
                    "configured node service user does not exist".to_string(),
                ));
            }
            break entry.pw_uid;
        }
    };

    let gid = {
        let mut entry = unsafe { std::mem::zeroed::<libc::group>() };
        let mut result = ptr::null_mut();
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let status = unsafe {
                libc::getgrnam_r(
                    group_name.as_ptr(),
                    &mut entry,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            };
            if status == libc::ERANGE {
                grow_principal_lookup_buffer(&mut buffer)?;
                continue;
            }
            if status != 0 {
                return Err(NodeError::InsecurePath(format!(
                    "failed to resolve configured node service group: {}",
                    io::Error::from_raw_os_error(status)
                )));
            }
            if result.is_null() {
                return Err(NodeError::InsecurePath(
                    "configured node service group does not exist".to_string(),
                ));
            }
            break entry.gr_gid;
        }
    };

    Ok(UnixOwner { uid, gid })
}

#[cfg(unix)]
fn owner_policy(
    platform: NodePlatform,
    custom_paths: bool,
    state: bool,
) -> Result<UnixOwner, NodeError> {
    use std::ffi::CString;

    if custom_paths {
        return Ok(UnixOwner {
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
        });
    }
    let service_name = match platform {
        NodePlatform::Linux => "omakure",
        NodePlatform::MacOs => "_omakure",
        NodePlatform::Windows => return Ok(UnixOwner { uid: 0, gid: 0 }),
    };
    let user_name = CString::new(service_name).expect("static principal has no NUL");
    let group_name = CString::new(service_name).expect("static principal has no NUL");
    let service_owner = lookup_unix_principal(&user_name, &group_name)?;
    if state {
        Ok(service_owner)
    } else {
        Ok(UnixOwner {
            uid: 0,
            gid: service_owner.gid,
        })
    }
}

#[cfg(not(unix))]
fn owner_policy(
    _platform: NodePlatform,
    _custom_paths: bool,
    _state: bool,
) -> Result<(), NodeError> {
    Ok(())
}

fn validate_directory_security(
    path: &Path,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] owner: (),
    _test_mode: bool,
) -> Result<(), NodeError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(path)?;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(NodeError::InsecurePath(format!(
                "{} must have mode 0700",
                path.display()
            )));
        }
        if metadata.uid() != owner.uid || metadata.gid() != owner.gid {
            return Err(NodeError::InsecurePath(format!(
                "{} has the wrong owner or group",
                path.display()
            )));
        }
    }
    #[cfg(windows)]
    validate_windows_security(path, true, _test_mode)?;
    #[cfg(not(unix))]
    let _ = owner;
    let _ = path;
    Ok(())
}

fn validate_file_security(
    path: &Path,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] owner: (),
    _test_mode: bool,
) -> Result<(), NodeError> {
    let metadata = fs::symlink_metadata(path)?;
    validate_file_security_metadata(path, &metadata, owner, _test_mode, 0o640)?;
    #[cfg(windows)]
    validate_windows_security(path, false, _test_mode)?;
    Ok(())
}

fn validate_file_security_mode(
    path: &Path,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] owner: (),
    _test_mode: bool,
    _expected_mode: u32,
) -> Result<(), NodeError> {
    let metadata = fs::symlink_metadata(path)?;
    validate_file_security_metadata(path, &metadata, owner, _test_mode, _expected_mode)?;
    #[cfg(windows)]
    validate_windows_security(path, false, _test_mode)?;
    Ok(())
}

/// Is `mode` no broader than `allowed`?
///
/// `allowed` is the *broadest* permission set a node file may carry, not the
/// only one it may carry. A stricter file is always acceptable: an operator
/// hardening `node.toml` from 0640 to 0600 has removed access, not granted it,
/// and refusing to read it turns a hardening step into an outage.
///
/// This cannot loosen a private file. The modes this admits for `allowed =
/// 0o600` are exactly the subsets of 0600 — 0000, 0200, 0400, 0600 — every one
/// of which is at least as strict as 0600, and no group or other bit can ever
/// pass. So one comparison serves both the public config and the private keys
/// without weakening either.
#[cfg(unix)]
fn mode_is_no_broader_than(mode: u32, allowed: u32) -> bool {
    mode & 0o777 & !allowed == 0
}

fn validate_file_security_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] owner: (),
    _test_mode: bool,
    expected_mode: u32,
) -> Result<(), NodeError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode() & 0o777;
        if !mode_is_no_broader_than(mode, expected_mode) {
            return Err(NodeError::InsecurePath(format!(
                "{} has mode {:04o}, which grants more access than the permitted {:04o}; \
                 run: chmod {:o} {}",
                path.display(),
                mode,
                expected_mode,
                expected_mode,
                path.display()
            )));
        }
        if metadata.uid() != owner.uid || metadata.gid() != owner.gid {
            return Err(NodeError::InsecurePath(format!(
                "{} is owned by {}:{} but must be owned by {}:{}; run: chown {}:{} {}",
                path.display(),
                metadata.uid(),
                metadata.gid(),
                owner.uid,
                owner.gid,
                owner.uid,
                owner.gid,
                path.display()
            )));
        }
    }
    let _ = (path, metadata, _test_mode, expected_mode);
    #[cfg(not(unix))]
    let _ = owner;
    Ok(())
}

#[cfg(windows)]
fn windows_has_reparse_point(path: &Path) -> Result<bool, NodeError> {
    use std::os::windows::ffi::OsStrExt;

    const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(NodeError::Io(io::Error::last_os_error()));
    }
    Ok(attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(windows)]
const OWNER_AND_DACL_SECURITY_INFORMATION: u32 = 0x0000_0005;
#[cfg(windows)]
const SE_FILE_OBJECT: u32 = 1;
#[cfg(windows)]
const ERROR_SUCCESS: u32 = 0;

#[cfg(windows)]
fn validate_windows_security(
    path: &Path,
    directory: bool,
    test_mode: bool,
) -> Result<(), NodeError> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    if windows_has_reparse_point(path)? {
        return Err(NodeError::UnsafePath(path.display().to_string()));
    }

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut security_descriptor: *mut std::ffi::c_void = ptr::null_mut();
    let mut owner_sid: *mut std::ffi::c_void = ptr::null_mut();
    let mut dacl: *mut std::ffi::c_void = ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_AND_DACL_SECURITY_INFORMATION,
            &mut owner_sid,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut security_descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(NodeError::InsecurePath(format!(
            "cannot read ACL for {} (error {status})",
            path.display()
        )));
    }
    validate_windows_security_descriptor(
        path,
        directory,
        test_mode,
        owner_sid,
        dacl,
        security_descriptor,
    )
}

#[cfg(windows)]
fn validate_windows_security_handle(
    path: &Path,
    file: &fs::File,
    test_mode: bool,
) -> Result<(), NodeError> {
    use std::os::windows::io::AsRawHandle;
    use std::ptr;

    let mut security_descriptor: *mut std::ffi::c_void = ptr::null_mut();
    let mut owner_sid: *mut std::ffi::c_void = ptr::null_mut();
    let mut dacl: *mut std::ffi::c_void = ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_AND_DACL_SECURITY_INFORMATION,
            &mut owner_sid,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut security_descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(NodeError::InsecurePath(format!(
            "cannot read ACL for {} (error {status})",
            path.display()
        )));
    }
    validate_windows_security_descriptor(
        path,
        false,
        test_mode,
        owner_sid,
        dacl,
        security_descriptor,
    )
}

#[cfg(windows)]
fn validate_windows_security_descriptor(
    path: &Path,
    directory: bool,
    test_mode: bool,
    owner_sid: *mut std::ffi::c_void,
    dacl: *mut std::ffi::c_void,
    security_descriptor: *mut std::ffi::c_void,
) -> Result<(), NodeError> {
    const ACL_SIZE_INFORMATION_CLASS: u32 = 2;
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const WIN_LOCAL_SYSTEM_SID: u32 = 22;
    const WIN_LOCAL_SERVICE_SID: u32 = 23;
    const SECURITY_MAX_SID_SIZE: usize = 68;
    use std::ptr;

    #[repr(C)]
    struct AceHeader {
        ace_type: u8,
        ace_flags: u8,
        ace_size: u16,
    }

    #[repr(C)]
    struct AclSizeInformation {
        ace_count: u32,
        acl_bytes_in_use: u32,
        acl_bytes_free: u32,
    }

    let result = (|| {
        if dacl.is_null() {
            return Err(NodeError::InsecurePath(format!(
                "{} has no explicit DACL",
                path.display()
            )));
        }
        if test_mode {
            return Ok(());
        }

        let mut acl_info = AclSizeInformation {
            ace_count: 0,
            acl_bytes_in_use: 0,
            acl_bytes_free: 0,
        };
        let acl_status = unsafe {
            GetAclInformation(
                dacl,
                &mut acl_info as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<AclSizeInformation>() as u32,
                ACL_SIZE_INFORMATION_CLASS,
            )
        };
        if acl_status == 0 || acl_info.ace_count == 0 {
            return Err(NodeError::InsecurePath(format!(
                "{} has an unreadable or empty DACL",
                path.display()
            )));
        }
        let mut allowed_system = [0u8; SECURITY_MAX_SID_SIZE];
        let mut allowed_service = [0u8; SECURITY_MAX_SID_SIZE];
        let mut system_size = allowed_system.len() as u32;
        let mut service_size = allowed_service.len() as u32;
        if unsafe {
            CreateWellKnownSid(
                WIN_LOCAL_SYSTEM_SID,
                ptr::null_mut(),
                allowed_system.as_mut_ptr() as *mut std::ffi::c_void,
                &mut system_size,
            ) == 0
                || CreateWellKnownSid(
                    WIN_LOCAL_SERVICE_SID,
                    ptr::null_mut(),
                    allowed_service.as_mut_ptr() as *mut std::ffi::c_void,
                    &mut service_size,
                ) == 0
        } {
            return Err(NodeError::InsecurePath(
                "cannot construct required Windows service SIDs".to_string(),
            ));
        }
        if owner_sid.is_null()
            || unsafe {
                EqualSid(
                    owner_sid,
                    allowed_system.as_mut_ptr() as *mut std::ffi::c_void,
                ) == 0
            }
        {
            return Err(NodeError::InsecurePath(format!(
                "{} has an unexpected owner",
                path.display()
            )));
        }
        let mut saw_system = false;
        let mut saw_service = false;
        for index in 0..acl_info.ace_count {
            let mut ace: *mut std::ffi::c_void = ptr::null_mut();
            if unsafe { GetAce(dacl, index, &mut ace) == 0 } || ace.is_null() {
                return Err(NodeError::InsecurePath(format!(
                    "cannot inspect ACL for {}",
                    path.display()
                )));
            }
            let header = unsafe { &*(ace as *const AceHeader) };
            if header.ace_type != ACCESS_ALLOWED_ACE_TYPE {
                return Err(NodeError::InsecurePath(format!(
                    "{} has a non-allow ACL entry",
                    path.display()
                )));
            }
            if header.ace_size
                < (std::mem::size_of::<AceHeader>() + std::mem::size_of::<u32>()) as u16
            {
                return Err(NodeError::InsecurePath(format!(
                    "{} has a malformed ACL entry",
                    path.display()
                )));
            }
            let sid = unsafe {
                (ace as *const u8)
                    .add(std::mem::size_of::<AceHeader>() + std::mem::size_of::<u32>())
                    as *mut std::ffi::c_void
            };
            let mask = unsafe {
                *((ace as *const u8).add(std::mem::size_of::<AceHeader>()) as *const u32)
            };
            let is_system =
                unsafe { EqualSid(sid, allowed_system.as_mut_ptr() as *mut std::ffi::c_void) != 0 };
            let is_service = unsafe {
                EqualSid(sid, allowed_service.as_mut_ptr() as *mut std::ffi::c_void) != 0
            };
            if !is_system && !is_service {
                return Err(NodeError::InsecurePath(format!(
                    "{} grants access to an unexpected principal",
                    path.display()
                )));
            }
            if !windows_security_access_allowed(directory, is_system, is_service, mask) {
                return Err(NodeError::InsecurePath(format!(
                    "{} grants an invalid access mask",
                    path.display()
                )));
            }
            saw_system |= is_system;
            saw_service |= is_service;
        }
        if !saw_system || !saw_service {
            return Err(NodeError::InsecurePath(format!(
                "{} must grant only LocalService and SYSTEM and include both",
                path.display()
            )));
        }
        Ok(())
    })();
    unsafe {
        LocalFree(security_descriptor);
    }
    result
}

#[cfg(windows)]
fn windows_security_access_allowed(
    directory: bool,
    is_system: bool,
    is_service: bool,
    mask: u32,
) -> bool {
    const FILE_GENERIC_READ: u32 = 0x0012_0089;
    const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
    const FILE_WRITABLE_SPECIFIC: u32 = 0x0000_0116;
    if is_system == is_service || mask & FILE_GENERIC_READ != FILE_GENERIC_READ {
        return false;
    }
    (is_system && mask & FILE_GENERIC_WRITE == FILE_GENERIC_WRITE)
        || (is_service && (directory || mask & FILE_WRITABLE_SPECIFIC == 0))
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetFileInformationByHandle(
        handle: *mut std::ffi::c_void,
        file_information: *mut ByHandleFileInformation,
    ) -> i32;
    fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

#[cfg(windows)]
#[link(name = "advapi32")]
extern "system" {
    fn GetSecurityInfo(
        handle: *mut std::ffi::c_void,
        object_type: u32,
        security_info: u32,
        owner: *mut *mut std::ffi::c_void,
        group: *mut *mut std::ffi::c_void,
        dacl: *mut *mut std::ffi::c_void,
        sacl: *mut *mut std::ffi::c_void,
        security_descriptor: *mut *mut std::ffi::c_void,
    ) -> u32;
    fn GetNamedSecurityInfoW(
        object_name: *const u16,
        object_type: u32,
        security_info: u32,
        owner: *mut *mut std::ffi::c_void,
        group: *mut *mut std::ffi::c_void,
        dacl: *mut *mut std::ffi::c_void,
        sacl: *mut *mut std::ffi::c_void,
        security_descriptor: *mut *mut std::ffi::c_void,
    ) -> u32;
    fn GetAclInformation(
        acl: *mut std::ffi::c_void,
        acl_information: *mut std::ffi::c_void,
        acl_information_length: u32,
        acl_information_class: u32,
    ) -> i32;
    fn GetAce(acl: *mut std::ffi::c_void, ace_index: u32, ace: *mut *mut std::ffi::c_void) -> i32;
    fn EqualSid(first: *mut std::ffi::c_void, second: *mut std::ffi::c_void) -> i32;
    fn CreateWellKnownSid(
        sid_type: u32,
        domain: *mut std::ffi::c_void,
        sid: *mut std::ffi::c_void,
        sid_size: *mut u32,
    ) -> i32;
}

// Keep the hand-written declarations tied to the Win32 ABI and documented
// parameter shapes when this module is compiled for Windows.
#[cfg(windows)]
const _: unsafe extern "system" fn(*const u16) -> u32 = GetFileAttributesW;
#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void, *mut ByHandleFileInformation) -> i32 =
    GetFileInformationByHandle;
#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void = LocalFree;
#[cfg(windows)]
const _: unsafe extern "system" fn(
    *mut std::ffi::c_void,
    u32,
    u32,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
) -> u32 = GetSecurityInfo;
#[cfg(windows)]
const _: unsafe extern "system" fn(
    *const u16,
    u32,
    u32,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
    *mut *mut std::ffi::c_void,
) -> u32 = GetNamedSecurityInfoW;
#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, u32, u32) -> i32 =
    GetAclInformation;
#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void, u32, *mut *mut std::ffi::c_void) -> i32 =
    GetAce;
#[cfg(windows)]
const _: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> i32 = EqualSid;
#[cfg(windows)]
const _: unsafe extern "system" fn(
    u32,
    *mut std::ffi::c_void,
    *mut std::ffi::c_void,
    *mut u32,
) -> i32 = CreateWellKnownSid;

/// Why a policy read produced the configuration it did.
///
/// Every variant below denies everything, and that is correct: a node that
/// cannot prove what it opted into has opted into nothing. But "nothing was
/// declared" and "the config exists and this node could not read it" are the
/// same *decision* and completely different operator problems. Collapsing them
/// is how one mode bit silently disables remote Cues, or the baseline gate,
/// with no distinguishable reason.
pub(crate) enum PolicyConfig {
    /// The config was read and parsed. Whatever it declares is what holds.
    Declared(Box<NodeConfig>),
    /// There is no config file. Nothing was declared.
    NothingDeclared,
    /// A config exists and could not be read or trusted. `String` says why, in
    /// the terms the operator needs: which file, and what is wrong with it.
    Unreadable(String),
}

/// Read this node's own public config for a policy decision, keeping the
/// reason for any failure rather than discarding it.
pub(crate) fn read_policy_config(context: &NodeContext) -> PolicyConfig {
    let mut file = match context.open_public_file() {
        Ok(Some(file)) => file,
        Ok(None) => return PolicyConfig::NothingDeclared,
        Err(error) => return PolicyConfig::Unreadable(error.to_string()),
    };
    let mut contents = String::new();
    if let Err(error) = file.read_to_string(&mut contents) {
        return PolicyConfig::Unreadable(format!(
            "{} could not be read: {error}",
            context.config_path().display()
        ));
    }
    match NodeConfig::parse(&contents) {
        Ok(config) => PolicyConfig::Declared(Box::new(config)),
        Err(error) => PolicyConfig::Unreadable(format!(
            "{} is not a usable node configuration: {error}",
            context.config_path().display()
        )),
    }
}

/// Report an unreadable policy config once per distinct reason.
///
/// Policy is read per session so a change takes effect without a restart, so an
/// unconditional warning would repeat for every inbound connection.
/// Deduplicating on the reason keeps a standing misconfiguration to one line
/// while still reporting a *new* problem when one appears.
pub(crate) fn warn_policy_unreadable(gate: &str, reason: &str) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};

    static REPORTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let reported = REPORTED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut reported = match reported.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if reported.insert(format!("{gate}\u{0}{reason}")) {
        eprintln!(
            "omakure: {gate} denied for every peer; this node's configuration could not be read ({reason})"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(debug_assertions)]
    use std::fs;

    #[cfg(debug_assertions)]
    fn test_context(root: &Path) -> NodeContext {
        NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(Some(root.join("state")), Some(root.join("node.toml"))),
            true,
            None,
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn platform_defaults_match_the_frozen_contract() {
        let linux = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::default(),
            false,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(linux.config_path(), Path::new("/etc/omakure/node.toml"));
        assert_eq!(linux.state_dir(), Path::new("/var/lib/omakure"));
        let mac = NodeContext::resolve_for(
            NodePlatform::MacOs,
            NodePathOverrides::default(),
            false,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            mac.config_path(),
            Path::new("/Library/Application Support/Omakure/node.toml")
        );
        assert_eq!(
            mac.state_dir(),
            Path::new("/Library/Application Support/Omakure")
        );
        let windows = NodeContext::resolve_for(
            NodePlatform::Windows,
            NodePathOverrides::default(),
            false,
            None,
            None,
            Some(PathBuf::from("/tmp/ProgramData")),
        )
        .unwrap();
        assert_eq!(
            windows.config_path(),
            Path::new("/tmp/ProgramData/Omakure/node.toml")
        );
        assert_eq!(windows.state_dir(), Path::new("/tmp/ProgramData/Omakure"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_principal_lookup_is_safe_under_concurrency() {
        use std::ffi::CStr;
        use std::ptr;
        use std::sync::{Arc, Barrier};

        const THREADS: usize = 16;
        let euid = unsafe { libc::geteuid() };
        let egid = unsafe { libc::getegid() };
        let (user_name, expected_uid) = {
            let mut entry = unsafe { std::mem::zeroed::<libc::passwd>() };
            let mut result = ptr::null_mut();
            let mut buffer = vec![0_u8; 16 * 1024];
            loop {
                let status = unsafe {
                    libc::getpwuid_r(
                        euid,
                        &mut entry,
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                        &mut result,
                    )
                };
                if status == libc::ERANGE {
                    grow_principal_lookup_buffer(&mut buffer).unwrap();
                    continue;
                }
                assert_eq!(status, 0, "resolve current euid");
                assert!(!result.is_null(), "current euid has a passwd entry");
                let name = unsafe { CStr::from_ptr(entry.pw_name) }.to_owned();
                break (name, entry.pw_uid as u32);
            }
        };
        let (group_name, expected_gid) = {
            let mut entry = unsafe { std::mem::zeroed::<libc::group>() };
            let mut result = ptr::null_mut();
            let mut buffer = vec![0_u8; 16 * 1024];
            loop {
                let status = unsafe {
                    libc::getgrgid_r(
                        egid,
                        &mut entry,
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                        &mut result,
                    )
                };
                if status == libc::ERANGE {
                    grow_principal_lookup_buffer(&mut buffer).unwrap();
                    continue;
                }
                assert_eq!(status, 0, "resolve current egid");
                assert!(!result.is_null(), "current egid has a group entry");
                let name = unsafe { CStr::from_ptr(entry.gr_name) }.to_owned();
                break (name, entry.gr_gid as u32);
            }
        };
        assert_eq!(expected_uid, euid as u32);
        assert_eq!(expected_gid, egid as u32);

        let barrier = Arc::new(Barrier::new(THREADS));
        let handles = (0..THREADS)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let user_name = user_name.clone();
                let group_name = group_name.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..256 {
                        let owner = lookup_unix_principal(&user_name, &group_name).unwrap();
                        assert_eq!(owner.uid, expected_uid);
                        assert_eq!(owner.gid, expected_gid);
                    }
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn every_platform_layout_can_initialize_with_shared_defaults() {
        let tmp = tempfile::TempDir::new().unwrap();
        for platform in [
            NodePlatform::Linux,
            NodePlatform::MacOs,
            NodePlatform::Windows,
        ] {
            let root = tmp.path().join(format!("{platform:?}"));
            fs::create_dir(&root).unwrap();
            let (state, config) = if platform == NodePlatform::Linux {
                (root.join("state"), root.join("node.toml"))
            } else {
                let state = root.join("state");
                (state.clone(), state.join("node.toml"))
            };
            let context = NodeContext::resolve_for(
                platform,
                NodePathOverrides::new(Some(state.clone()), Some(config.clone())),
                true,
                None,
                None,
                None,
            )
            .unwrap();
            context.initialize(&NodeConfig::default()).unwrap();
            assert!(state.is_dir());
            assert!(config.is_file());
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn cli_overrides_env_and_env_overrides_defaults() {
        let layout = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(
                Some(PathBuf::from("/tmp/cli-state")),
                Some(PathBuf::from("/tmp/cli.toml")),
            ),
            true,
            Some(PathBuf::from("/tmp/env-state")),
            Some(PathBuf::from("/tmp/env.toml")),
            None,
        )
        .unwrap();
        assert_eq!(layout.state_dir(), Path::new("/tmp/cli-state"));
        assert_eq!(layout.config_path(), Path::new("/tmp/cli.toml"));
        let env_only = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::default(),
            true,
            Some(PathBuf::from("/tmp/env-state")),
            Some(PathBuf::from("/tmp/env.toml")),
            None,
        )
        .unwrap();
        assert_eq!(env_only.state_dir(), Path::new("/tmp/env-state"));
        assert!(NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(
                Some(PathBuf::from("/tmp/cli-state")),
                Some(PathBuf::from("/tmp/cli.toml")),
            ),
            false,
            None,
            None,
            None,
        )
        .is_err());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn constructor_has_no_filesystem_side_effects_and_resolves_identity_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = test_context(tmp.path());
        assert!(!tmp.path().join("state").exists());
        assert!(!tmp.path().join("node.toml").exists());
        assert_eq!(
            context.identity_path(),
            tmp.path().join("state/identity.key")
        );
        assert_eq!(
            context.database_path(),
            tmp.path().join("state/node.sqlite")
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn initialization_is_explicit_atomic_and_does_not_create_identity_or_database() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = test_context(tmp.path());
        let result = context.initialize(&NodeConfig::default()).unwrap();
        assert!(result.state_dir_created);
        assert!(result.config_created);
        assert!(context.state_dir().is_dir());
        assert!(context.config_path().is_file());
        assert!(!context.identity_path().exists());
        assert!(!context.database_path().exists());
        assert!(NodeConfig::parse(&fs::read_to_string(context.config_path()).unwrap()).is_ok());
        let second = context.initialize(&NodeConfig::default()).unwrap();
        assert!(!second.state_dir_created);
        assert!(!second.config_created);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn initialization_rejects_missing_parents_and_symlink_boundaries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(
                Some(tmp.path().join("missing/state")),
                Some(tmp.path().join("missing/node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(matches!(
            missing.initialize(&NodeConfig::default()),
            Err(NodeError::UnsafePath(_))
        ));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = tempfile::TempDir::new().unwrap();
            let link = tmp.path().join("link");
            symlink(outside.path(), &link).unwrap();
            let context = NodeContext::resolve_for(
                NodePlatform::Linux,
                NodePathOverrides::new(
                    Some(link.join("state")),
                    Some(tmp.path().join("node.toml")),
                ),
                true,
                None,
                None,
                None,
            )
            .unwrap();
            assert!(matches!(
                context.initialize(&NodeConfig::default()),
                Err(NodeError::UnsafePath(_))
            ));
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn failed_config_initialization_cleans_up_a_new_state_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = tmp.path().join("state");
        let config = tmp.path().join("node.toml");
        fs::write(&config, "version = 2\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let context = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(Some(state.clone()), Some(config)),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(context.initialize(&NodeConfig::default()).is_err());
        assert!(!state.exists());
    }

    #[test]
    fn production_rejects_test_environment_overrides() {
        let error = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::default(),
            false,
            Some(PathBuf::from("/tmp/state")),
            Some(PathBuf::from("/tmp/node.toml")),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, NodeError::TestOverrideOutsideTestMode));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn unsafe_paths_are_rejected_without_io() {
        for (state, config) in [
            (PathBuf::from("relative"), PathBuf::from("/tmp/node.toml")),
            (
                PathBuf::from("/tmp/state/../bad"),
                PathBuf::from("/tmp/node.toml"),
            ),
            (PathBuf::from("/tmp/state"), PathBuf::from("relative.toml")),
        ] {
            assert!(NodeContext::resolve_for(
                NodePlatform::Linux,
                NodePathOverrides::new(Some(state), Some(config)),
                true,
                None,
                None,
                None,
            )
            .is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn opened_file_identity_detects_path_replacement() {
        let tmp = tempfile::TempDir::new().unwrap();
        let opened_path = tmp.path().join("opened.toml");
        let replacement_path = tmp.path().join("replacement.toml");
        fs::write(&opened_path, "opened").unwrap();
        fs::write(&replacement_path, "replacement").unwrap();
        let file = fs::File::open(&opened_path).unwrap();
        assert!(validate_open_file_identity(&opened_path, &file).is_ok());
        assert!(validate_open_file_identity(&replacement_path, &file).is_err());
    }

    #[cfg(all(unix, debug_assertions))]
    #[test]
    fn private_bounded_file_reader_enforces_path_mode_and_size() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let tmp = tempfile::TempDir::new().unwrap();
        let context = test_context(tmp.path());
        let token = tmp.path().join("bootstrap.token");
        fs::write(&token, b"bounded").unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        let lease = context.stage_private_bounded_file(&token, 7).unwrap();
        assert_eq!(lease.contents(), b"bounded");
        lease.restore().unwrap();
        assert!(context.stage_private_bounded_file(&token, 6).is_err());
        fs::set_permissions(&token, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(context.stage_private_bounded_file(&token, 7).is_err());
        fs::remove_file(&token).unwrap();

        let target = tmp.path().join("real.token");
        fs::write(&target, b"secret").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &token).unwrap();
        assert!(context.stage_private_bounded_file(&token, 6).is_err());
    }

    #[cfg(all(unix, debug_assertions))]
    #[test]
    fn startup_tombstone_scan_is_bounded() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let context = test_context(tmp.path());
        let token = tmp.path().join("bootstrap.token");
        for index in 0..11 {
            let tombstone = tmp.path().join(format!(
                "{PRIVATE_TOKEN_TOMBSTONE_PREFIX}{index:032x}-bootstrap.token"
            ));
            fs::write(&tombstone, b"t").unwrap();
            fs::set_permissions(&tombstone, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let tombstones = context.list_private_token_tombstones(&token, 1).unwrap();
        assert_eq!(tombstones.len(), PRIVATE_TOKEN_TOMBSTONE_RETRY_LIMIT);
        for tombstone in tombstones {
            fs::remove_file(tombstone.tombstone_path).unwrap();
        }
        for entry in fs::read_dir(tmp.path()).unwrap() {
            let entry = entry.unwrap();
            fs::remove_file(entry.path()).unwrap();
        }
    }

    #[cfg(all(windows, debug_assertions))]
    #[test]
    fn windows_drive_prefix_is_accepted_but_parent_components_are_not() {
        let valid = NodeContext::resolve_for(
            NodePlatform::Windows,
            NodePathOverrides::new(
                Some(PathBuf::from(r"C:\ProgramData\Omakure")),
                Some(PathBuf::from(r"C:\ProgramData\Omakure\node.toml")),
            ),
            true,
            None,
            None,
            None,
        );
        assert!(valid.is_ok());
        let unsafe_path = NodeContext::resolve_for(
            NodePlatform::Windows,
            NodePathOverrides::new(
                Some(PathBuf::from(r"C:\ProgramData\..\Omakure")),
                Some(PathBuf::from(r"C:\ProgramData\Omakure\node.toml")),
            ),
            true,
            None,
            None,
            None,
        );
        assert!(unsafe_path.is_err());
    }

    #[cfg(all(windows, debug_assertions))]
    #[test]
    fn windows_native_prefix_is_accepted_for_simulated_unix_layouts() {
        for platform in [NodePlatform::Linux, NodePlatform::MacOs] {
            let result = NodeContext::resolve_for(
                platform,
                NodePathOverrides::new(
                    Some(PathBuf::from(r"C:\Temp\Omakure")),
                    Some(PathBuf::from(r"C:\Temp\Omakure\node.toml")),
                ),
                true,
                None,
                None,
                None,
            );
            assert!(result.is_ok(), "simulated {platform:?} layout");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_lifecycle_lock_sharing_errors_are_contention() {
        for code in [32, 33] {
            assert!(is_lifecycle_lock_contention(&io::Error::from_raw_os_error(
                code
            )));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_service_acl_policy_allows_only_required_principals_and_access() {
        const READ: u32 = 0x0012_0089;
        const WRITE: u32 = 0x0012_0116;
        const WRITE_DAC: u32 = 0x0004_0000;
        assert!(windows_security_access_allowed(
            false,
            false,
            true,
            READ | WRITE_DAC
        ));
        assert!(windows_security_access_allowed(
            false,
            true,
            false,
            READ | WRITE
        ));
        assert!(windows_security_access_allowed(false, false, true, READ));
        assert!(windows_security_access_allowed(
            true,
            false,
            true,
            READ | WRITE
        ));
        assert!(!windows_security_access_allowed(
            false,
            false,
            true,
            READ | WRITE
        ));
        assert!(!windows_security_access_allowed(false, false, false, READ));
        assert!(!windows_security_access_allowed(false, true, true, READ));
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_build_rejects_test_mode_even_when_requested() {
        let error = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::default(),
            true,
            Some(PathBuf::from("/tmp/state")),
            Some(PathBuf::from("/tmp/node.toml")),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, NodeError::TestModeUnavailable));
    }

    /// Hardening a file must never be an outage.
    ///
    /// 0640 is the *broadest* a public node config may be, not the only mode it
    /// may have. An operator who chmods `node.toml` to 0600 has removed access,
    /// not granted it, and the node must keep reading it. The previous exact
    /// comparison refused 0600 and 0400 alongside 0644, so tightening the
    /// config broke the node.
    #[cfg(all(unix, debug_assertions))]
    #[test]
    fn a_public_config_may_be_hardened_but_never_loosened() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("node.toml");
        fs::write(&path, "version = 1\n").unwrap();
        let owner = owner_policy(NodePlatform::Linux, true, false).unwrap();

        // Nothing beyond owner-rw plus group-r may pass. Checked first so a
        // regression reports the widened access, not a stricter-mode edge case.
        for mode in 0..=0o777u32 {
            if mode & !0o640 == 0 {
                continue;
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                validate_file_security(&path, owner, true).is_err(),
                "mode {mode:04o} grants access beyond 0640 and must be refused"
            );
        }

        // Every stricter mode must be readable: hardening is not an outage.
        // 0600 and 0400 are the cases the exact comparison used to refuse.
        for mode in 0..=0o777u32 {
            if mode & !0o640 != 0 {
                continue;
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                validate_file_security(&path, owner, true).is_ok(),
                "mode {mode:04o} is no broader than 0640 and must be accepted"
            );
        }
    }

    /// The same comparison guards `identity.key`, `authority.key` and
    /// `publisher.key` at 0600. Relaxing "exactly" to "no broader than" must
    /// not make *those* loosenable.
    ///
    /// Proven exhaustively rather than by sample: all 512 permission modes are
    /// checked, and acceptance must hold for exactly the subsets of 0600. That
    /// is the whole security argument for using one comparison for both files —
    /// no group bit and no other bit can ever pass on a private file.
    #[cfg(all(unix, debug_assertions))]
    #[test]
    fn no_group_or_other_bit_can_ever_pass_on_a_private_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let context = test_context(temp.path());
        let path = temp.path().join("identity.key");
        fs::write(&path, b"key").unwrap();

        // The security invariant first, so a regression reports the exposure
        // rather than some stricter-mode edge case that happens to sort lower.
        for mode in 0..=0o777u32 {
            if mode & 0o077 == 0 {
                continue;
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                context.validate_private_file(&path).is_err(),
                "mode {mode:04o} exposes a private key to group or other and must be refused"
            );
        }

        // Then the exact accepted set: the subsets of 0600 and nothing else.
        for mode in 0..=0o777u32 {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(
                context.validate_private_file(&path).is_ok(),
                mode & !0o600 == 0,
                "mode {mode:04o} against a 0600 private file"
            );
        }
    }

    /// The refusal must lead somewhere. Naming neither the file nor the mode
    /// nor the expectation leaves the operator with no route from the error to
    /// the fix, and every file this validator guards produced the same
    /// sentence.
    #[cfg(all(unix, debug_assertions))]
    #[test]
    fn a_refused_mode_names_the_file_the_mode_and_the_remedy() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("node.toml");
        fs::write(&path, "version = 1\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let owner = owner_policy(NodePlatform::Linux, true, false).unwrap();

        let error = validate_file_security(&path, owner, true).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains(&path.display().to_string()),
            "the refusal must name the file: {message}"
        );
        assert!(
            message.contains("0644"),
            "the refusal must name the mode it found: {message}"
        );
        assert!(
            message.contains("0640"),
            "the refusal must name what it permits: {message}"
        );
        assert!(
            message.contains("chmod 640"),
            "the refusal must carry the remedy: {message}"
        );
    }

    /// Deny-all is right for every unreadable config. Silence about *which*
    /// failure it was is not.
    ///
    /// A mode bit that turns remote Cues off must not be indistinguishable from
    /// a node that simply never opted in — those are the same decision and
    /// completely different operator problems, and the reason is the only thing
    /// that tells them apart.
    #[cfg(all(unix, debug_assertions))]
    #[test]
    fn an_unreadable_policy_config_is_distinguishable_from_nothing_declared() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let context = test_context(temp.path());
        let path = temp.path().join("node.toml");
        let valid = NodeConfig::default().to_toml().unwrap();

        // No config at all: nothing was declared.
        assert!(matches!(
            read_policy_config(&context),
            PolicyConfig::NothingDeclared
        ));

        // The passing control. Without it the assertions below would hold just
        // as well if the reader never returned anything but a failure.
        fs::write(&path, &valid).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(matches!(
            read_policy_config(&context),
            PolicyConfig::Declared(_)
        ));

        // A mode the node refuses: unreadable, and the reason names the file
        // and the mode so the operator can act on it.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let PolicyConfig::Unreadable(mode_reason) = read_policy_config(&context) else {
            panic!("a config this node refuses to read must not look like nothing declared");
        };
        assert!(
            mode_reason.contains(&path.display().to_string()) && mode_reason.contains("0644"),
            "the reason must name the file and the mode: {mode_reason}"
        );

        // A different failure must read differently, or the reason carries no
        // information beyond "something went wrong".
        fs::write(&path, "this is not toml = = =").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let PolicyConfig::Unreadable(parse_reason) = read_policy_config(&context) else {
            panic!("a config that will not parse must not look like nothing declared");
        };
        assert!(
            parse_reason.contains(&path.display().to_string()),
            "the reason must name the file: {parse_reason}"
        );
        assert_ne!(
            mode_reason, parse_reason,
            "a permissions failure and a malformed config must not read the same"
        );
    }
}
