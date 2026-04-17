use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::ports::ScriptRepository;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchStatus {
    Idle,
    Indexing,
    Ready { script_count: usize },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub script_path: PathBuf,
    pub display_name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub schema_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchField {
    pub name: String,
    pub prompt: Option<String>,
    pub kind: String,
    pub required: bool,
}

#[derive(Debug, Clone)]
pub struct SearchDetails {
    pub display_name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub fields: Vec<SearchField>,
    pub schema_error: Option<String>,
}

#[derive(Clone)]
pub struct SearchIndex {
    db_path: PathBuf,
    status: Arc<Mutex<SearchStatus>>,
}

impl SearchIndex {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            status: Arc::new(Mutex::new(SearchStatus::Idle)),
        }
    }

    #[cfg(test)]
    pub fn new_in_memory() -> Self {
        Self {
            db_path: PathBuf::from(":memory:"),
            status: Arc::new(Mutex::new(SearchStatus::Idle)),
        }
    }

    pub fn status(&self) -> SearchStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or(SearchStatus::Error(
                "Search status lock poisoned".to_string(),
            ))
    }

    pub fn start_background_rebuild(&self, root: PathBuf) {
        let status = self.status.clone();
        let db_path = self.db_path.clone();
        thread::spawn(move || {
            let _ = update_status(&status, SearchStatus::Indexing);
            match rebuild_index(&db_path, &root) {
                Ok(count) => {
                    let _ = update_status(
                        &status,
                        SearchStatus::Ready {
                            script_count: count,
                        },
                    );
                }
                Err(err) => {
                    let _ = update_status(&status, SearchStatus::Error(err));
                }
            }
        });
    }

    pub fn query(&self, query: &str) -> Result<Vec<SearchResult>, String> {
        let conn = open_connection(&self.db_path)?;
        init_db(&conn)?;

        let tokens = split_query(query);
        let mut sql = String::from(
            "SELECT script_path, display_name, description, tags, schema_error \
             FROM script_index",
        );
        if !tokens.is_empty() {
            sql.push_str(" WHERE ");
            for (idx, _) in tokens.iter().enumerate() {
                if idx > 0 {
                    sql.push_str(" AND ");
                }
                sql.push_str("search_blob LIKE ? ESCAPE '\\'");
            }
        }
        sql.push_str(" ORDER BY display_name COLLATE NOCASE, script_path COLLATE NOCASE");

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| format!("Search prepare failed: {}", err))?;

        let params: Vec<String> = tokens
            .iter()
            .map(|token| format!("%{}%", escape_like(token)))
            .collect();
        let rows = stmt
            .query_map(params_from_iter(params), |row| {
                let script_path: String = row.get(0)?;
                let display_name: String = row.get(1)?;
                let description: Option<String> = row.get(2)?;
                let tags_raw: Option<String> = row.get(3)?;
                let schema_error: Option<String> = row.get(4)?;
                Ok(SearchResult {
                    script_path: PathBuf::from(script_path),
                    display_name,
                    description,
                    tags: parse_tags(tags_raw),
                    schema_error,
                })
            })
            .map_err(|err| format!("Search query failed: {}", err))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|err| format!("Search row failed: {}", err))?);
        }
        Ok(results)
    }

    pub fn load_details(&self, script_path: &Path) -> Result<Option<SearchDetails>, String> {
        let conn = open_connection(&self.db_path)?;
        init_db(&conn)?;
        let script_path = script_path.to_string_lossy().to_string();

        let mut stmt = conn
            .prepare(
                "SELECT display_name, description, tags, schema_error \
                 FROM script_index WHERE script_path = ?",
            )
            .map_err(|err| format!("Search detail prepare failed: {}", err))?;

        let base = stmt
            .query_row([script_path.clone()], |row| {
                let display_name: String = row.get(0)?;
                let description: Option<String> = row.get(1)?;
                let tags_raw: Option<String> = row.get(2)?;
                let schema_error: Option<String> = row.get(3)?;
                Ok((display_name, description, tags_raw, schema_error))
            })
            .optional()
            .map_err(|err| format!("Search detail query failed: {}", err))?;

        let (display_name, description, tags_raw, schema_error) = match base {
            Some(base) => base,
            None => return Ok(None),
        };

        let mut field_stmt = conn
            .prepare(
                "SELECT name, prompt, kind, required \
                 FROM script_fields WHERE script_path = ? \
                 ORDER BY field_order",
            )
            .map_err(|err| format!("Search fields prepare failed: {}", err))?;

        let rows = field_stmt
            .query_map([script_path], |row| {
                Ok(SearchField {
                    name: row.get(0)?,
                    prompt: row.get(1)?,
                    kind: row.get(2)?,
                    required: row.get::<_, i64>(3)? != 0,
                })
            })
            .map_err(|err| format!("Search fields query failed: {}", err))?;

        let mut fields = Vec::new();
        for row in rows {
            fields.push(row.map_err(|err| format!("Search field row failed: {}", err))?);
        }

        Ok(Some(SearchDetails {
            display_name,
            description,
            tags: parse_tags(tags_raw),
            fields,
            schema_error,
        }))
    }
}

pub(crate) fn rebuild_index(db_path: &Path, root: &Path) -> Result<usize, String> {
    let repo = FsWorkspaceRepository::new(root.to_path_buf());
    let scripts = repo
        .list_scripts_recursive()
        .map_err(|err| format!("List scripts failed: {}", err))?;

    let mut conn = open_connection(db_path)?;
    init_db(&conn)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|err| format!("Enable foreign keys failed: {}", err))?;

    let tx = conn
        .transaction()
        .map_err(|err| format!("Begin transaction failed: {}", err))?;
    tx.execute("DELETE FROM script_fields", [])
        .map_err(|err| format!("Clear fields failed: {}", err))?;
    tx.execute("DELETE FROM script_index", [])
        .map_err(|err| format!("Clear scripts failed: {}", err))?;

    for script in &scripts {
        let relative = script.strip_prefix(root).unwrap_or(script);
        let relative_str = relative.to_string_lossy().to_string();
        let file_name = script
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("script");

        let mut schema_error = None;
        let mut display_name = file_name.to_string();
        let mut description: Option<String> = None;
        let mut tags: Vec<String> = Vec::new();
        let mut fields: Vec<SearchField> = Vec::new();

        match repo.read_schema(script) {
            Ok(schema) => {
                display_name = schema.name.clone();
                description = schema.description.clone();
                tags = schema.tags.clone().unwrap_or_default();
                fields = schema
                    .fields
                    .iter()
                    .map(|field| SearchField {
                        name: field.name.clone(),
                        prompt: field.prompt.clone(),
                        kind: field.kind.clone(),
                        required: field.required.unwrap_or(false),
                    })
                    .collect();
            }
            Err(err) => {
                schema_error = Some(err.to_string());
            }
        }

        let search_blob = build_search_blob(
            &relative_str,
            &display_name,
            description.as_deref(),
            &tags,
            &fields,
        );

        let tags_raw = if tags.is_empty() {
            None
        } else {
            Some(tags.join(","))
        };
        let indexed_at = timestamp_ms();

        tx.execute(
            "INSERT OR REPLACE INTO script_index \
             (script_path, display_name, description, tags, search_blob, schema_error, indexed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                relative_str.as_str(),
                display_name,
                description,
                tags_raw,
                search_blob,
                schema_error,
                indexed_at
            ],
        )
        .map_err(|err| format!("Insert script failed: {}", err))?;

        for (order, field) in fields.iter().enumerate() {
            tx.execute(
                "INSERT INTO script_fields \
                 (script_path, field_order, name, prompt, kind, required) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    &relative_str,
                    order as i64,
                    &field.name,
                    field.prompt.clone(),
                    &field.kind,
                    if field.required { 1 } else { 0 }
                ],
            )
            .map_err(|err| format!("Insert field failed: {}", err))?;
        }
    }

    tx.commit()
        .map_err(|err| format!("Commit search index failed: {}", err))?;
    Ok(scripts.len())
}

fn open_connection(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Create search db folder failed: {}", err))?;
    }
    let conn =
        Connection::open(db_path).map_err(|err| format!("Open search db failed: {}", err))?;
    conn.busy_timeout(Duration::from_millis(500))
        .map_err(|err| format!("Search db busy timeout failed: {}", err))?;
    let _journal_mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(|err| format!("Enable WAL failed: {}", err))?;
    Ok(conn)
}

fn init_db(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS script_index (\
            script_path TEXT PRIMARY KEY,\
            display_name TEXT NOT NULL,\
            description TEXT,\
            tags TEXT,\
            search_blob TEXT NOT NULL,\
            schema_error TEXT,\
            indexed_at INTEGER NOT NULL\
        );\
        CREATE TABLE IF NOT EXISTS script_fields (\
            script_path TEXT NOT NULL,\
            field_order INTEGER NOT NULL,\
            name TEXT NOT NULL,\
            prompt TEXT,\
            kind TEXT,\
            required INTEGER NOT NULL,\
            FOREIGN KEY(script_path) REFERENCES script_index(script_path) ON DELETE CASCADE\
        );\
        CREATE INDEX IF NOT EXISTS idx_script_search ON script_index(search_blob);\
        CREATE INDEX IF NOT EXISTS idx_script_fields ON script_fields(script_path);",
    )
    .map_err(|err| format!("Init search db failed: {}", err))
}

pub(crate) fn build_search_blob(
    script_path: &str,
    display_name: &str,
    description: Option<&str>,
    tags: &[String],
    fields: &[SearchField],
) -> String {
    let mut parts = Vec::new();
    parts.push(script_path.to_string());
    parts.push(display_name.to_string());
    if let Some(description) = description {
        parts.push(description.to_string());
    }
    for tag in tags {
        parts.push(tag.clone());
    }
    for field in fields {
        parts.push(field.name.clone());
        if let Some(prompt) = &field.prompt {
            parts.push(prompt.clone());
        }
        parts.push(field.kind.clone());
    }
    parts.join(" ").to_lowercase()
}

pub(crate) fn split_query(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(|token| token.to_lowercase())
        .collect()
}

pub(crate) fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub(crate) fn parse_tags(tags_raw: Option<String>) -> Vec<String> {
    let Some(tags_raw) = tags_raw else {
        return Vec::new();
    };
    tags_raw
        .split(',')
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .map(|tag| tag.to_string())
        .collect()
}

fn update_status(status: &Arc<Mutex<SearchStatus>>, next: SearchStatus) -> Result<(), ()> {
    if let Ok(mut guard) = status.lock() {
        *guard = next;
        Ok(())
    } else {
        Err(())
    }
}

fn timestamp_ms() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use tempfile::TempDir;

    // --- Pure helper tests ---

    #[test]
    fn test_build_search_blob_basic() {
        let result = build_search_blob("path/script.sh", "My Script", Some("desc"), &[], &[]);
        assert_eq!(result, "path/script.sh my script desc");
    }

    #[test]
    fn test_build_search_blob_no_description() {
        let result = build_search_blob("s.sh", "S", None, &[], &[]);
        assert_eq!(result, "s.sh s");
    }

    #[test]
    fn test_build_search_blob_with_tags_and_fields() {
        let tags = vec!["deploy".to_string(), "infra".to_string()];
        let fields = vec![SearchField {
            name: "env".to_string(),
            prompt: Some("Environment".to_string()),
            kind: "string".to_string(),
            required: true,
        }];
        let blob = build_search_blob("s.sh", "S", None, &tags, &fields);
        assert!(blob.contains("deploy"));
        assert!(blob.contains("infra"));
        assert!(blob.contains("env"));
        assert!(blob.contains("environment"));
        assert!(blob.contains("string"));
    }

    #[rstest]
    #[case::two_tokens("hello world", vec!["hello", "world"])]
    #[case::trimmed("  spaced  ", vec!["spaced"])]
    #[case::empty("", vec![])]
    #[case::lowercased("UPPER Case", vec!["upper", "case"])]
    fn test_split_query(#[case] input: &str, #[case] expected: Vec<&str>) {
        let result = split_query(input);
        let expected: Vec<String> = expected.into_iter().map(String::from).collect();
        assert_eq!(result, expected);
    }

    #[rstest]
    #[case::no_special_chars("hello", "hello")]
    #[case::percent_escaped("50%", "50\\%")]
    #[case::underscore_escaped("under_score", "under\\_score")]
    #[case::backslash_escaped("back\\slash", "back\\\\slash")]
    fn test_escape_like(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(escape_like(input), expected);
    }

    #[test]
    fn test_parse_tags_csv() {
        assert_eq!(parse_tags(Some("a,b,c".to_string())), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_tags_single() {
        assert_eq!(parse_tags(Some("single".to_string())), vec!["single"]);
    }

    #[test]
    fn test_parse_tags_trimmed() {
        assert_eq!(
            parse_tags(Some(" spaced , tags ".to_string())),
            vec!["spaced", "tags"]
        );
    }

    #[test]
    fn test_parse_tags_none() {
        let result: Vec<String> = parse_tags(None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_tags_empty_string() {
        let result: Vec<String> = parse_tags(Some("".to_string()));
        assert!(result.is_empty());
    }

    // --- SQLite integration tests ---

    #[test]
    fn test_query_empty_index() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.sqlite");
        let index = SearchIndex::new(db);
        let results = index.query("anything").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_rebuild_and_query() {
        let tmp = TempDir::new().unwrap();
        let scripts_dir = tmp.path().join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();

        fs::write(
            scripts_dir.join("deploy.sh"),
            r#"#!/bin/bash
# OMAKURE_SCHEMA_START
# {"Name": "Deploy App", "Description": "Deploy to production", "Tags": ["deploy", "infra"], "Fields": []}
# OMAKURE_SCHEMA_END
echo deploying
"#,
        )
        .unwrap();

        fs::write(scripts_dir.join("test.sh"), "#!/bin/bash\necho testing").unwrap();

        let db = tmp.path().join("search.sqlite");
        let count = rebuild_index(&db, &scripts_dir).unwrap();
        assert_eq!(count, 2);

        let index = SearchIndex::new(db);

        // Query by name
        let results = index.query("deploy").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display_name, "Deploy App");
        assert_eq!(results[0].tags, vec!["deploy", "infra"]);

        // Query all
        let all = index.query("").unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_load_details() {
        let tmp = TempDir::new().unwrap();
        let scripts_dir = tmp.path().join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();

        fs::write(
            scripts_dir.join("setup.sh"),
            r#"#!/bin/bash
# OMAKURE_SCHEMA_START
# {"Name": "Setup", "Fields": [{"Name": "env", "Type": "string", "Order": 0, "Required": true}]}
# OMAKURE_SCHEMA_END
echo setup
"#,
        )
        .unwrap();

        let db = tmp.path().join("search.sqlite");
        rebuild_index(&db, &scripts_dir).unwrap();

        let index = SearchIndex::new(db);
        let details = index.load_details(Path::new("setup.sh")).unwrap().unwrap();
        assert_eq!(details.display_name, "Setup");
        assert_eq!(details.fields.len(), 1);
        assert_eq!(details.fields[0].name, "env");
        assert!(details.fields[0].required);
    }

    #[test]
    fn test_load_details_not_found() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("search.sqlite");
        let index = SearchIndex::new(db);
        let _ = index.query(""); // initialize DB
        let details = index.load_details(Path::new("nonexistent.sh")).unwrap();
        assert!(details.is_none());
    }

    #[test]
    fn test_status_lifecycle() {
        let index = SearchIndex::new(PathBuf::from(":memory:"));
        assert_eq!(index.status(), SearchStatus::Idle);
    }

    #[test]
    fn test_new_in_memory_constructor() {
        let index = SearchIndex::new_in_memory();
        assert_eq!(index.status(), SearchStatus::Idle);
    }

    #[test]
    fn test_query_with_multiple_tokens_uses_and() {
        let tmp = TempDir::new().unwrap();
        let scripts_dir = tmp.path().join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::write(
            scripts_dir.join("a.sh"),
            r#"#!/bin/bash
# OMAKURE_SCHEMA_START
# {"Name": "Alpha Beta", "Description": "double word", "Fields": []}
# OMAKURE_SCHEMA_END
"#,
        )
        .unwrap();
        let db = tmp.path().join("search.sqlite");
        rebuild_index(&db, &scripts_dir).unwrap();
        let index = SearchIndex::new(db);

        let hit = index.query("alpha beta").unwrap();
        assert_eq!(hit.len(), 1);

        let miss = index.query("alpha gamma").unwrap();
        assert!(miss.is_empty());
    }
}
