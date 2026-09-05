fn handler() {
    use crate::cli::run as run_cli;
    use rusqlite::Connection;
    run_cli();
    let _ = Connection::open_in_memory();
}
