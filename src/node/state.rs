use super::context::NodeContext;
#[cfg(unix)]
use super::fs_unix::{create_secure_directory, owner_policy};
use super::layout::{
    AUTHORITY_KEY_FILE, DATABASE_FILE, IDENTITY_KEY_FILE, IDENTITY_LOCK_FILE, LIFECYCLE_LOCK_FILE,
    PUBLISHER_KEY_FILE, TRANSPORT_CERTIFICATE_FILE, TRANSPORT_KEY_FILE,
};
#[cfg(not(unix))]
use super::security::{create_secure_directory, owner_policy};
use super::security::{
    ensure_safe_parent, set_directory_mode, symlink_metadata_if_present,
    validate_directory_security, validate_file_security, write_new_file_atomically,
};
use super::NodeError;
use crate::domain::NodeConfig;
use std::fs;
use std::io::{self};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInitialization {
    pub state_dir_created: bool,
    pub config_created: bool,
}

impl NodeContext {
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
            ensure_safe_parent(config_parent, self.test_mode)?;
        }

        let state_dir_created = self.ensure_state_directory()?;

        if shared_config {
            ensure_safe_parent(config_parent, self.test_mode)?;
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
                    let created = match write_new_file_atomically(
                        self.config_path(),
                        contents.as_bytes(),
                        0o640,
                    ) {
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
        ensure_safe_parent(self.state_dir(), self.test_mode)?;
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
            let metadata = match symlink_metadata_if_present(&entry.path())? {
                Some(metadata) => metadata,
                None => continue,
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(NodeError::UnexpectedFileType(name.into_owned()));
            }
            let allowed = matches!(
                name.as_ref(),
                IDENTITY_KEY_FILE
                    // The enrollment-authority signing key, on a node that
                    // issues fleet membership. Added to this closed list
                    // deliberately: the list is a security control, and a new
                    // entry is an amendment to it, not a convenience.
                    | AUTHORITY_KEY_FILE
                    // The baseline-publisher signing key, on a node that ships
                    // code to the fleet. Second amendment to this closed list,
                    // held to the same standard as the first: the list is the
                    // control, and every entry is a decision to admit one more
                    // file to the node's private state.
                    | PUBLISHER_KEY_FILE
                    | DATABASE_FILE
                    | "node.sqlite-wal"
                    | "node.sqlite-shm"
                    | TRANSPORT_KEY_FILE
                    | TRANSPORT_CERTIFICATE_FILE
                    | IDENTITY_LOCK_FILE
                    | LIFECYCLE_LOCK_FILE
                    | "node.toml"
            ) || name
                .strip_prefix(".cue-execution-")
                .and_then(|digest| digest.strip_suffix(".lock"))
                .is_some_and(|digest| digest.len() == 64 && crate::util::hex::is_lower(digest));
            if !allowed {
                return Err(NodeError::InsecurePath(format!(
                    "unsupported node state entry {name:?}"
                )));
            }
        }
        Ok(true)
    }
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
