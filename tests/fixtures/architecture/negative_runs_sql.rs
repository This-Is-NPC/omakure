fn inspect(conn: &rusqlite::Connection) {
    let _ = conn.query_row(
        concat!("SELECT run_id FROM ", "runs WHERE run_id = ?"),
        [],
        |_| Ok(()),
    );
}
