use super::*;

#[test]
fn enrollment_cleanup_is_bounded_and_caps_are_frozen() {
    assert_eq!(MAX_ENROLLMENT_REPLAY_ROWS, 1_000_000);
    assert_eq!(MAX_ENROLLMENT_AUDIT_ROWS, 1_000_000);
    assert_eq!(MAX_ENROLLMENT_REQUEST_ROWS, 1_000_000);
    assert_eq!(MAX_BOOTSTRAP_PROOF_ROWS, 1_000_000);
    assert_eq!(MAX_ENROLLMENT_CLEANUP_ROWS, 10_000);

    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let _registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    let mut connection = Connection::open(context.database_path()).unwrap();
    configure_connection(&mut connection).unwrap();
    let transaction = connection.transaction().unwrap();
    for value in 0_u64..=10_000 {
        let mut replay_id = [0_u8; 16];
        replay_id[..8].copy_from_slice(&value.to_be_bytes());
        transaction
            .execute(
                "INSERT INTO enrollment_replays
                     (replay_kind, replay_id, expires_at, first_seen)
                     VALUES ('bundle', ?1, 1, 1)",
                [&replay_id[..]],
            )
            .unwrap();
    }
    cleanup_enrollment_replays(&transaction, 2).unwrap();
    transaction.commit().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM enrollment_replays", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
}
