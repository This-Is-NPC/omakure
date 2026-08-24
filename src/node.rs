use crate::domain::{NodeConfig, NodeConfigError};
use crate::node_identity::NodeIdentityStatus;
use crate::node_registry::{NodeRegistry, RegistryError};
use rand::rngs::OsRng;
use rand::RngCore;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

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

    pub fn identity_path(&self) -> PathBuf {
        self.state_dir.join("identity.key")
    }

    pub fn database_path(&self) -> PathBuf {
        self.state_dir.join("node.sqlite")
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

    pub fn identity_path(&self) -> PathBuf {
        self.layout.identity_path()
    }

    pub fn database_path(&self) -> PathBuf {
        self.layout.database_path()
    }

    pub fn is_test_mode(&self) -> bool {
        self.test_mode
    }

    pub fn open_trust_registry(
        &self,
        identity: &NodeIdentityStatus,
    ) -> Result<NodeRegistry, RegistryError> {
        NodeRegistry::open(self, identity)
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
                    write_atomic_config(self.config_path(), contents.as_bytes())?;
                    validate_file_security(
                        self.config_path(),
                        owner_policy(self.platform, self.custom_paths, false)?,
                        self.test_mode,
                    )?;
                    Ok(true)
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

    pub(crate) fn validate_private_file(&self, path: &Path) -> Result<(), NodeError> {
        validate_file_security_mode(
            path,
            owner_policy(self.platform, self.custom_paths, true)?,
            self.test_mode,
            0o600,
        )
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
                return Err(NodeError::InsecurePath(
                    "node configuration file could not be opened securely".to_string(),
                ))
            }
        };
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(NodeError::UnexpectedFileType(
                "node configuration".to_string(),
            ));
        }
        validate_file_security_metadata(
            &metadata,
            owner_policy(self.platform, self.custom_paths, false)?,
            self.test_mode,
            0o640,
        )?;
        if !ensure_safe_parent_if_present(path)? {
            return Err(NodeError::InsecurePath(
                "node configuration path changed while opening".to_string(),
            ));
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
    let service_name = CString::new(service_name).expect("static principal has no NUL");
    let (uid, gid) = unsafe {
        let passwd = libc::getpwnam(service_name.as_ptr());
        let group = libc::getgrnam(service_name.as_ptr());
        if passwd.is_null() || group.is_null() {
            return Err(NodeError::InsecurePath(
                "configured node service principal does not exist".to_string(),
            ));
        }
        ((*passwd).pw_uid, (*group).gr_gid)
    };
    if state {
        Ok(UnixOwner { uid, gid })
    } else {
        Ok(UnixOwner { uid: 0, gid })
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
    #[cfg(not(unix))] _owner: (),
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
    let _ = path;
    Ok(())
}

fn validate_file_security(
    path: &Path,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] _owner: (),
    _test_mode: bool,
) -> Result<(), NodeError> {
    let metadata = fs::symlink_metadata(path)?;
    validate_file_security_metadata(&metadata, owner, _test_mode, 0o640)?;
    #[cfg(windows)]
    validate_windows_security(path, false, _test_mode)?;
    Ok(())
}

fn validate_file_security_mode(
    path: &Path,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] _owner: (),
    _test_mode: bool,
    _expected_mode: u32,
) -> Result<(), NodeError> {
    let metadata = fs::symlink_metadata(path)?;
    validate_file_security_metadata(&metadata, owner, _test_mode, _expected_mode)?;
    #[cfg(windows)]
    validate_windows_security(path, false, _test_mode)?;
    Ok(())
}

fn validate_file_security_metadata(
    metadata: &fs::Metadata,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] _owner: (),
    _test_mode: bool,
    expected_mode: u32,
) -> Result<(), NodeError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o777 != expected_mode {
            return Err(NodeError::InsecurePath(
                "node file has an unexpected mode".to_string(),
            ));
        }
        if metadata.uid() != owner.uid || metadata.gid() != owner.gid {
            return Err(NodeError::InsecurePath(
                "node file has the wrong owner or group".to_string(),
            ));
        }
    }
    let _ = (metadata, _test_mode, expected_mode);
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
    _directory: bool,
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
    validate_windows_security_descriptor(path, test_mode, owner_sid, dacl, security_descriptor)
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
    validate_windows_security_descriptor(path, test_mode, owner_sid, dacl, security_descriptor)
}

#[cfg(windows)]
fn validate_windows_security_descriptor(
    path: &Path,
    test_mode: bool,
    owner_sid: *mut std::ffi::c_void,
    dacl: *mut std::ffi::c_void,
    security_descriptor: *mut std::ffi::c_void,
) -> Result<(), NodeError> {
    const ACL_SIZE_INFORMATION_CLASS: u32 = 2;
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const INHERITED_ACE: u8 = 0x10;
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
                    && EqualSid(
                        owner_sid,
                        allowed_service.as_mut_ptr() as *mut std::ffi::c_void,
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
            if header.ace_type != ACCESS_ALLOWED_ACE_TYPE || header.ace_flags & INHERITED_ACE != 0 {
                return Err(NodeError::InsecurePath(format!(
                    "{} has a non-explicit or non-allow ACL entry",
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
}
