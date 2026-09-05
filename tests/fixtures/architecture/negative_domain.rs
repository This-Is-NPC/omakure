fn load_schema(path: &std::path::Path) -> String {
    use crate::adapters::workspace_repository::FsWorkspaceRepository as Repo;
    use std::fs::read_to_string as read;
    let _ = Repo::new;
    read(path).expect("schema")
}
