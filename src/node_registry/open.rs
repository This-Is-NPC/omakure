use super::error::RegistryError;
use super::fields::validate_identity;
use super::schema::create_schema;
use super::validate::validate_schema;
use super::{NodeRegistry, BUSY_TIMEOUT, SCHEMA_VERSION};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentityStatus;
use crate::util::sqlite::{is_lock_contention, OPEN_RETRY_DELAYS};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// One writer per database per process.
///
/// Every mutation opens its own connection and begins `IMMEDIATE`, so writers
/// in one process meet on SQLite's file lock and wait under `BUSY_TIMEOUT`.
/// That budget is for another process holding the database; spent on threads
/// of this one it turns a queue into a deadline, and a slow disk hands the
/// last writer in line `SQLITE_BUSY` for work it would have finished a moment
/// later. In-process writers queue here, where waiting has no timeout, and
/// the busy handler keeps the cross-process case it was written for.
static WRITE_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl NodeRegistry {
    /// Open and validate the node-owned database for the supplied public identity.
    pub(crate) fn open(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
    ) -> Result<Self, RegistryError> {
        Self::open_with_mode(context, identity, false)
    }

    pub(crate) fn open_for_initialization(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
    ) -> Result<Self, RegistryError> {
        Self::open_with_mode(context, identity, true)
    }

    fn open_with_mode(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
        allow_create: bool,
    ) -> Result<Self, RegistryError> {
        context.ensure_state_directory()?;
        let path = context.database_path();
        let database_existed = std::fs::symlink_metadata(&path).is_ok();
        if !database_existed && !allow_create {
            return Err(RegistryError::NotFound(
                "node trust registry is not initialized".to_string(),
            ));
        }
        let sidecars_existed = database_sidecar_presence(&path);
        let (node_id, public_key) = validate_identity(identity)?;
        let registry = Self {
            path,
            publisher_key_path: context.publisher_key_path(),
            local_node_id: node_id,
            local_public_key: public_key,
            schema_mutation_allowed: true,
            read_connection: None,
        };
        registry.with_mutating_connection(|connection| {
            if !database_existed {
                set_new_database_mode(&registry.path)?;
            }
            for (sidecar, existed) in database_sidecar_paths(&registry.path)
                .into_iter()
                .zip(sidecars_existed)
            {
                if !existed && sidecar.exists() {
                    set_new_database_mode(&sidecar)?;
                }
            }
            validate_database_security(context, &registry.path)?;
            initialize_database(connection, &registry)
        })?;
        Ok(registry)
    }
    /// Open and validate an existing registry without creating state or
    /// mutating its schema. Schema creation belongs to the
    /// serialized node initialization path (`open`).
    pub fn open_existing(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
    ) -> Result<Self, RegistryError> {
        Self::open_existing_with_integrity(context, identity, true, false)
    }

    /// Open an existing registry for observational read surfaces shared by
    /// Health and public node status.
    ///
    /// This performs the same filesystem, identity, and schema validation as
    /// [`Self::open_existing`] but deliberately omits the full integrity scan.
    pub(crate) fn open_health_observational(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
    ) -> Result<Self, RegistryError> {
        Self::open_existing_with_integrity(context, identity, false, true)
    }

    fn open_existing_with_integrity(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
        run_integrity_check: bool,
        reuse_read_connection: bool,
    ) -> Result<Self, RegistryError> {
        if !context.validate_existing_state_directory()? {
            return Err(RegistryError::NotFound(
                crate::node::STATE_NOT_INITIALIZED.to_string(),
            ));
        }
        let path = context.database_path();
        match std::fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() =>
            {
                return Err(RegistryError::InvalidSchema(
                    "node.sqlite has an unexpected file type".to_string(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(RegistryError::NotFound(
                    "node trust registry is not initialized".to_string(),
                ));
            }
            Err(error) => return Err(error.into()),
        }
        let (node_id, public_key) = validate_identity(identity)?;
        validate_database_security(context, &path)?;
        let registry = Self {
            path,
            publisher_key_path: context.publisher_key_path(),
            local_node_id: node_id,
            local_public_key: public_key,
            schema_mutation_allowed: false,
            read_connection: None,
        };
        let mut connection =
            Connection::open_with_flags(&registry.path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure_connection_observational(&mut connection)?;
        validate_database_security(context, &registry.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        if run_integrity_check {
            integrity_check(&transaction)?;
        }
        validate_existing_database(&transaction, &registry)?;
        transaction.commit()?;
        let read_connection = if reuse_read_connection {
            Some(Arc::new(Mutex::new(connection)))
        } else {
            None
        };
        Ok(Self {
            read_connection,
            ..registry
        })
    }

    pub(super) fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        if let Some(connection) = &self.read_connection {
            let mut guard = connection
                .lock()
                .expect("observational registry connection lock");
            return operation(&mut guard);
        }
        let mut connection = self.open_configured()?;
        operation(&mut connection)
    }

    /// Open and configure a connection, waiting out a transient lock.
    fn open_configured(&self) -> Result<Connection, RegistryError> {
        let mut attempt = 0;
        loop {
            let opened = Connection::open(&self.path)
                .map_err(RegistryError::from)
                .and_then(|mut connection| {
                    if self.schema_mutation_allowed {
                        configure_connection(&mut connection)?;
                    } else {
                        configure_connection_read_only(&mut connection)?;
                    }
                    Ok(connection)
                });
            match opened {
                Ok(connection) => return Ok(connection),
                Err(error) if is_transient_lock(&error) && attempt < OPEN_RETRY_DELAYS.len() => {
                    std::thread::sleep(OPEN_RETRY_DELAYS[attempt]);
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(super) fn with_mutating_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        let lock = write_lock_for(&self.path);
        // A writer that panicked mid-transaction rolled back when its
        // connection dropped; the poisoned flag says nothing about the file.
        let _writer = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut connection = self.open_configured()?;
        let result = operation(&mut connection)?;
        checkpoint_wal(&connection)?;
        Ok(result)
    }
}

/// A lock another connection holds right now, as opposed to any other error.
pub(super) fn is_transient_lock(error: &RegistryError) -> bool {
    matches!(
        error,
        RegistryError::Sqlite(inner) if is_lock_contention(inner)
    )
}

fn write_lock_for(path: &Path) -> Arc<Mutex<()>> {
    let mut locks = WRITE_LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(locks.entry(path.to_path_buf()).or_default())
}

fn checkpoint_wal(connection: &Connection) -> Result<(), RegistryError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
    Ok(())
}

pub(super) fn ignore_vanished_private_file(
    result: Result<(), crate::node::NodeError>,
) -> Result<(), crate::node::NodeError> {
    match result {
        Err(error) if crate::node::is_not_found(&error) => Ok(()),
        other => other,
    }
}

fn validate_database_security(context: &NodeContext, path: &Path) -> Result<(), RegistryError> {
    context.validate_private_file(path)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = path.with_file_name(format!(
            "{}{}",
            path.file_name().unwrap().to_string_lossy(),
            suffix
        ));
        match std::fs::symlink_metadata(&sidecar) {
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() =>
            {
                return Err(RegistryError::InvalidSchema(
                    "node SQLite sidecar has an unexpected file type".to_string(),
                ));
            }
            Ok(_) => ignore_vanished_private_file(context.validate_private_file(&sidecar))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn database_sidecar_paths(path: &Path) -> [PathBuf; 2] {
    [
        path.with_file_name(format!(
            "{}-wal",
            path.file_name().unwrap().to_string_lossy()
        )),
        path.with_file_name(format!(
            "{}-shm",
            path.file_name().unwrap().to_string_lossy()
        )),
    ]
}

fn database_sidecar_presence(path: &Path) -> [bool; 2] {
    database_sidecar_paths(path).map(|path| std::fs::symlink_metadata(path).is_ok())
}

fn set_new_database_mode(path: &Path) -> Result<(), RegistryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

fn validate_existing_database(
    connection: &Connection,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 0 {
        return Err(RegistryError::InvalidSchema(
            "existing node.sqlite has no schema version".to_string(),
        ));
    }
    ensure_current_schema_version(version)?;
    validate_schema(connection, registry)
}

fn initialize_database(
    connection: &mut Connection,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    integrity_check(connection)?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 0 {
        if has_user_objects(connection)? {
            return Err(RegistryError::InvalidSchema(
                "unversioned database contains objects".to_string(),
            ));
        }
        let transaction = connection.transaction()?;
        create_schema(&transaction, registry)?;
        transaction.commit()?;
    } else {
        ensure_current_schema_version(version)?;
    }
    validate_schema(connection, registry)
}

/// Only the current schema is accepted. Older registries are not migrated:
/// the node refuses to open them rather than guess at their rows.
fn ensure_current_schema_version(version: i64) -> Result<(), RegistryError> {
    if version > SCHEMA_VERSION {
        return Err(RegistryError::InvalidSchema(format!(
            "database version {version} is newer than supported version {SCHEMA_VERSION}"
        )));
    }
    if version < SCHEMA_VERSION {
        return Err(RegistryError::InvalidSchema(format!(
            "database version {version} is older than supported version {SCHEMA_VERSION}; \
             older node registries are not migrated"
        )));
    }
    Ok(())
}

pub(super) fn configure_connection(connection: &mut Connection) -> Result<(), RegistryError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let mut journal_mode: String =
        connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        journal_mode = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    }
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(RegistryError::InvalidSchema(format!(
            "journal mode is {journal_mode:?}, expected WAL"
        )));
    }
    connection.execute_batch("PRAGMA foreign_keys = ON")?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(RegistryError::InvalidSchema(
            "foreign key enforcement is disabled".to_string(),
        ));
    }
    Ok(())
}

fn configure_connection_read_only(connection: &mut Connection) -> Result<(), RegistryError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let journal_mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(RegistryError::InvalidSchema(format!(
            "journal mode is {journal_mode:?}, expected WAL"
        )));
    }
    connection.execute_batch("PRAGMA foreign_keys = ON")?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(RegistryError::InvalidSchema(
            "foreign key enforcement is disabled".to_string(),
        ));
    }
    Ok(())
}

fn configure_connection_observational(connection: &mut Connection) -> Result<(), RegistryError> {
    configure_connection_read_only(connection)?;
    connection.execute_batch("PRAGMA query_only = ON")?;
    let query_only: i64 = connection.query_row("PRAGMA query_only", [], |row| row.get(0))?;
    if query_only != 1 {
        return Err(RegistryError::InvalidSchema(
            "SQLite query-only mode could not be enabled".to_string(),
        ));
    }
    Ok(())
}

fn integrity_check(connection: &Connection) -> Result<(), RegistryError> {
    let result: String = connection.query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))?;
    if result != "ok" {
        return Err(RegistryError::Corrupt(result));
    }
    Ok(())
}

fn has_user_objects(connection: &Connection) -> Result<bool, RegistryError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type IN ('table', 'index', 'trigger', 'view') AND name NOT LIKE 'sqlite_%')",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0)
}
