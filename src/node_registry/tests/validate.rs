use super::*;

#[test]
fn older_schema_versions_fail_closed_without_mutation() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    drop(NodeRegistry::open(&context, identity.public_status()).unwrap());
    let connection = Connection::open(context.database_path()).unwrap();
    connection
        .execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION - 1))
        .unwrap();
    drop(connection);

    for result in [
        NodeRegistry::open(&context, identity.public_status()),
        NodeRegistry::open_existing(&context, identity.public_status()),
    ] {
        match result {
            Err(RegistryError::InvalidSchema(message)) => {
                assert!(message.contains("older than supported"), "{message}");
            }
            other => panic!("older schema must fail closed: {other:?}"),
        }
    }
    let connection = Connection::open(context.database_path()).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION - 1);
}

#[test]
fn future_schema_corruption_and_metadata_downgrade_fail_closed() {
    let temp = TempDir::new().unwrap();
    let ctx = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&ctx).unwrap();
    let database = ctx.database_path();
    let connection = Connection::open(&database).unwrap();
    // One past whatever this build supports: the property is that a
    // database written by a newer Omakure is refused, and pinning the
    // literal here would quietly stop testing that at the next version bump.
    connection
        .execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
        .unwrap();
    assert!(matches!(
        NodeRegistry::open(&ctx, identity.public_status()),
        Err(RegistryError::InvalidSchema(_))
    ));
    drop(connection);
    fs::remove_file(&database).unwrap();
    assert!(matches!(
        NodeRegistry::open(&ctx, identity.public_status()),
        Err(RegistryError::NotFound(_))
    ));
    assert!(ctx.identity_path().is_file());
    assert!(!ctx.database_path().exists());

    let temp = TempDir::new().unwrap();
    let context2 = node_context(temp.path());
    let identity2 = NodeIdentity::load_or_initialize(&context2).unwrap();
    let connection = Connection::open(context2.database_path()).unwrap();

    connection
        .execute(
            "UPDATE metadata SET value = '0' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        NodeRegistry::open(&context2, identity2.public_status()),
        Err(RegistryError::InvalidSchema(_))
    ));
}
