use super::*;

#[test]
fn ignore_vanished_private_file_skips_not_found_only() {
    use std::io;

    let not_found = crate::node::NodeError::Io(io::Error::new(io::ErrorKind::NotFound, "gone"));
    assert!(ignore_vanished_private_file(Err(not_found)).is_ok());

    let insecure = crate::node::NodeError::InsecurePath("bad".into());
    assert!(matches!(
        ignore_vanished_private_file(Err(insecure)),
        Err(crate::node::NodeError::InsecurePath(_))
    ));
    assert!(ignore_vanished_private_file(Ok(())).is_ok());
}

#[test]
fn open_health_observational_tolerates_vanished_sqlite_sidecars() {
    let temp = TempDir::new().unwrap();
    let node_context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&node_context).unwrap();
    let registry = NodeRegistry::open(&node_context, identity.public_status()).unwrap();
    drop(registry);

    let wal = node_context
        .database_path()
        .with_file_name("node.sqlite-wal");
    fs::write(&wal, b"wal").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wal, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let status = identity.public_status();
    assert!(NodeRegistry::open_health_observational(&node_context, status).is_ok());

    if wal.exists() {
        fs::remove_file(&wal).unwrap();
    }
    assert!(NodeRegistry::open_health_observational(&node_context, status).is_ok());
}

#[test]
fn initializes_reopens_and_keeps_runs_path_separate() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    assert!(registry.path().ends_with("node.sqlite"));
    assert!(!registry.path().ends_with("runs.sqlite"));
    assert_eq!(
        NodeRegistry::open(&context, identity.public_status())
            .unwrap()
            .peers()
            .unwrap()
            .len(),
        0
    );
    let connection = Connection::open(context.database_path()).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
}

#[test]
fn observational_registry_reads_succeed_while_writer_is_reserved() {
    let temp = TempDir::new().unwrap();
    let node_context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&node_context).unwrap();
    let registry = NodeRegistry::open(&node_context, identity.public_status()).unwrap();
    registry
        .record_transport_audit(
            "snapshot_probe",
            &identity.public_status().node_id,
            None,
            None,
            0,
            "accepted",
            None,
        )
        .unwrap();
    let mut writer = Connection::open(node_context.database_path()).unwrap();
    configure_connection(&mut writer).unwrap();
    let writer_transaction = writer
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let status = identity.public_status();
    let registry = NodeRegistry::open_existing(&node_context, status).unwrap();

    assert!(registry.peer(&status.node_id).is_ok());
    assert!(registry.peers().is_ok());
    assert!(registry.peers_limited(1).is_ok());
    assert!(registry.revocations().is_ok());
    assert!(registry.audit_events().is_ok());
    assert!(registry
        .transport_peer(&status.node_id, &status.public_key_hex)
        .is_ok());

    writer_transaction.rollback().unwrap();
}

#[test]
fn only_a_held_lock_counts_as_transient_when_opening() {
    let busy = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
        Some("database is locked".to_string()),
    );
    let locked = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_LOCKED),
        None,
    );
    let not_a_database = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
        None,
    );
    assert!(is_transient_lock(&RegistryError::Sqlite(busy)));
    assert!(is_transient_lock(&RegistryError::Sqlite(locked)));
    assert!(!is_transient_lock(&RegistryError::Sqlite(not_a_database)));
    assert!(!is_transient_lock(&RegistryError::Corrupt("x".into())));
}

#[test]
fn a_connection_opens_through_a_briefly_held_exclusive_lock() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    let path = context.database_path();
    let holder = thread::spawn(move || {
        let mut connection = Connection::open(path).unwrap();
        connection
            .execute_batch("PRAGMA locking_mode = EXCLUSIVE")
            .unwrap();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        thread::sleep(Duration::from_millis(150));
        transaction.rollback().unwrap();
    });
    thread::sleep(Duration::from_millis(20));
    assert!(
        registry.peers().is_ok(),
        "a read must wait out another process's lock rather than fail"
    );
    holder.join().unwrap();
}

#[test]
fn open_existing_succeeds_after_clean_close_without_sidecars() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    {
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        registry
            .record_transport_audit(
                "cold_open_probe",
                &identity.public_status().node_id,
                None,
                None,
                0,
                "accepted",
                None,
            )
            .unwrap();
    }

    for sidecar in database_sidecar_paths(&context.database_path()) {
        assert!(
            !sidecar.exists(),
            "clean close left SQLite sidecar behind: {}",
            sidecar.display()
        );
    }

    let reopened = NodeRegistry::open_existing(&context, identity.public_status()).unwrap();
    assert!(reopened.peers().is_ok());
}

#[test]
fn open_existing_rejects_corrupt_index() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    NodeRegistry::open(&context, identity.public_status()).unwrap();

    let connection = Connection::open(context.database_path()).unwrap();
    connection
        .execute_batch(
            "PRAGMA writable_schema = ON;
                 UPDATE sqlite_master
                    SET rootpage = 1
                  WHERE type = 'index' AND name = 'peers_state_idx';
                 PRAGMA writable_schema = OFF;",
        )
        .unwrap();
    drop(connection);

    let observational = NodeRegistry::open_health_observational(&context, identity.public_status());
    assert!(observational.is_ok(), "{observational:?}");

    let result = NodeRegistry::open_existing(&context, identity.public_status());
    assert!(
        matches!(result, Err(RegistryError::Corrupt(_))),
        "{result:?}"
    );
}
