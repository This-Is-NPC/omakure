macro_rules! query {
    ($sql:literal) => {
        $sql
    };
}

fn allowed_storage() {
    let _ = query!("SELECT id FROM search_index WHERE term = ?");
    let _ = "runs are execution records";
}
