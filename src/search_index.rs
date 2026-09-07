use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::ports::ScriptRepository;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub script_path: PathBuf,
    pub display_name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub field_count: usize,
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
#[allow(dead_code)] // detail projection: constructed by `load_details`, asserted by tests
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
}

impl SearchIndex {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    /// Refresh and read one committed snapshot. A failed refresh never serves stale data.
    pub fn search(&self, root: &Path, query: &str) -> Result<Vec<SearchResult>, String> {
        let mut conn = open_connection(&self.db_path)?;
        conn.execute("PRAGMA foreign_keys = ON", [])
            .map_err(|err| format!("Enable foreign keys failed: {err}"))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("Begin search transaction failed: {err}"))?;
        init_db(&tx)?;
        rebuild_index(&tx, root)?;
        let results = Self::query_connection(&tx, query)?;
        tx.commit()
            .map_err(|err| format!("Commit search index failed: {err}"))?;
        Ok(results)
    }

    #[cfg(test)]
    pub fn query(&self, query: &str) -> Result<Vec<SearchResult>, String> {
        let conn = open_connection(&self.db_path)?;
        init_db(&conn)?;
        Self::query_connection(&conn, query)
    }

    fn query_connection(conn: &Connection, query: &str) -> Result<Vec<SearchResult>, String> {
        let tokens = split_query(query);
        let mut sql = String::from(
            "SELECT script_path, display_name, description, tags, schema_error, \
             (SELECT COUNT(*) FROM script_fields sf WHERE sf.script_path = script_index.script_path) \
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
                let field_count: i64 = row.get(5)?;
                Ok(SearchResult {
                    script_path: PathBuf::from(script_path),
                    display_name,
                    description,
                    tags: parse_tags(tags_raw),
                    field_count: usize::try_from(field_count).unwrap_or(0),
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

    #[allow(dead_code)]
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

fn rebuild_index(tx: &Connection, root: &Path) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("Canonicalize search root failed: {error}"))?;
    let repo = FsWorkspaceRepository::new(root.clone());
    let scripts = repo
        .list_scripts_recursive()
        .map_err(|err| format!("List scripts failed: {}", err))?;

    tx.execute("DELETE FROM script_fields", [])
        .map_err(|err| format!("Clear fields failed: {}", err))?;
    tx.execute("DELETE FROM script_index", [])
        .map_err(|err| format!("Clear scripts failed: {}", err))?;

    for script in &scripts {
        let relative_str = logical_relative_path(script, &root);
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

    Ok(())
}

fn logical_relative_path(path: &Path, root: &Path) -> String {
    let path_text = path.to_string_lossy().replace('\\', "/");
    let root_text = root
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    path_text
        .strip_prefix(&root_text)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(&path_text)
        .to_string()
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
    fn logical_relative_paths_use_forward_slashes_for_windows_fixtures() {
        let root = Path::new(r"C:\workspace\scripts");
        let path = Path::new(r"C:\workspace\scripts\tools\deploy.sh");

        assert_eq!(logical_relative_path(path, root), "tools/deploy.sh");
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
    fn failed_refresh_rolls_back_and_does_not_return_stale_results() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("scripts");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("old.sh"), "no schema").unwrap();
        let db = tmp.path().join("search.sqlite");
        let index = SearchIndex::new(db.clone());
        assert_eq!(index.search(&root, "old").unwrap().len(), 1);
        let conn = open_connection(&db).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_insert BEFORE INSERT ON script_index
             BEGIN SELECT RAISE(ABORT, 'injected refresh failure'); END;",
        )
        .unwrap();
        fs::remove_file(root.join("old.sh")).unwrap();
        fs::write(root.join("new.sh"), "no schema").unwrap();

        let err = index.search(&root, "old").unwrap_err();
        assert!(err.contains("injected refresh failure"), "{err}");
        assert_eq!(index.query("old").unwrap().len(), 1);
        assert!(index.query("new").unwrap().is_empty());
        conn.execute_batch("DROP TRIGGER reject_insert").unwrap();
        let results = index.search(&root, "").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].script_path, Path::new("new.sh"));
        assert!(results[0].schema_error.is_some());
    }

    #[test]
    fn commit_failure_does_not_return_uncommitted_results() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("scripts");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("old.sh"), "no schema").unwrap();
        let db = tmp.path().join("search.sqlite");
        let index = SearchIndex::new(db.clone());
        index.search(&root, "").unwrap();
        let conn = open_connection(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE commit_guard (
                script_path TEXT REFERENCES script_index(script_path)
                DEFERRABLE INITIALLY DEFERRED
             );
             CREATE TRIGGER fail_commit AFTER INSERT ON script_index
             BEGIN INSERT INTO commit_guard VALUES ('missing.sh'); END;",
        )
        .unwrap();
        fs::remove_file(root.join("old.sh")).unwrap();
        fs::write(root.join("new.sh"), "no schema").unwrap();
        let err = index.search(&root, "new").unwrap_err();
        assert!(err.contains("Commit search index failed"), "{err}");
        assert_eq!(index.query("old").unwrap().len(), 1);
        assert!(index.query("new").unwrap().is_empty());
    }

    #[test]
    fn locked_index_returns_an_error_instead_of_cached_results() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("scripts");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("old.sh"), "no schema").unwrap();
        let db = tmp.path().join("search.sqlite");
        let index = SearchIndex::new(db.clone());
        index.search(&root, "").unwrap();
        let mut conn = open_connection(&db).unwrap();
        let lock = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let err = index.search(&root, "old").unwrap_err();
        assert!(err.contains("locked"), "{err}");
        lock.rollback().unwrap();
        assert_eq!(index.search(&root, "old").unwrap().len(), 1);
    }

    #[test]
    fn concurrent_searches_return_complete_snapshots() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("scripts");
        fs::create_dir(&root).unwrap();
        for name in ["a.sh", "b.sh", "c.sh"] {
            fs::write(root.join(name), "no schema").unwrap();
        }
        let index = SearchIndex::new(tmp.path().join("search.sqlite"));
        // Initialize WAL before testing writer serialization.
        index.search(&root, "").unwrap();
        let barrier = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..3)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        index.search(&root, "").map(|results| {
                            results
                                .into_iter()
                                .map(|entry| entry.script_path)
                                .collect::<Vec<_>>()
                        })
                    })
                })
                .collect();
            let mut completed = 0;
            for handle in handles {
                match handle.join().unwrap() {
                    Ok(paths) => {
                        completed += 1;
                        assert_eq!(
                            paths,
                            vec![
                                PathBuf::from("a.sh"),
                                PathBuf::from("b.sh"),
                                PathBuf::from("c.sh")
                            ]
                        );
                    }
                    // Contention is bounded, including on a heavily loaded CI runner.
                    Err(err) => assert!(err.contains("database is locked"), "{err}"),
                }
            }
            assert!(completed > 0, "at least one writer must complete");
        });
    }

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
        let index = SearchIndex::new(db);
        assert_eq!(index.search(&scripts_dir, "").unwrap().len(), 2);

        // Query by name
        let results = index.query("deploy").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display_name, "Deploy App");
        assert_eq!(results[0].tags, vec!["deploy", "infra"]);

        // Query all
        let all = index.query("").unwrap();
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .any(|result| result.script_path == Path::new("deploy.sh")));
    }

    #[test]
    fn test_rebuild_honors_omakureignore() {
        let tmp = TempDir::new().unwrap();
        let scripts_dir = tmp.path().join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();

        fs::write(
            scripts_dir.join("visible.sh"),
            r#"#!/bin/bash
# OMAKURE_SCHEMA_START
# {"Name": "Visible Script", "Description": "shown", "Fields": []}
# OMAKURE_SCHEMA_END
"#,
        )
        .unwrap();
        fs::write(
            scripts_dir.join("hidden.sh"),
            r#"#!/bin/bash
# OMAKURE_SCHEMA_START
# {"Name": "Hidden Script", "Description": "ignored", "Fields": []}
# OMAKURE_SCHEMA_END
"#,
        )
        .unwrap();
        fs::write(scripts_dir.join(".omakureignore"), "hidden.sh\n").unwrap();

        let db = tmp.path().join("search.sqlite");
        let index = SearchIndex::new(db);
        assert_eq!(index.search(&scripts_dir, "").unwrap().len(), 1);
        assert_eq!(index.query("visible").unwrap().len(), 1);
        assert!(index.query("hidden").unwrap().is_empty());
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
        let index = SearchIndex::new(db);
        index.search(&scripts_dir, "").unwrap();
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
        let index = SearchIndex::new(db);
        index.search(&scripts_dir, "").unwrap();

        let hit = index.query("alpha beta").unwrap();
        assert_eq!(hit.len(), 1);

        let miss = index.query("alpha gamma").unwrap();
        assert!(miss.is_empty());
    }
}
