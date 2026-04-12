use mlua::{Lua, Table, Value};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct WidgetData {
    pub title: String,
    pub lines: Vec<String>,
}

pub fn load_widget(dir: &Path) -> Result<Option<WidgetData>, String> {
    let script_path = dir.join("index.lua");
    if !script_path.is_file() {
        return Ok(None);
    }

    let script = fs::read_to_string(&script_path)
        .map_err(|err| format!("Failed to read {}: {}", script_path.display(), err))?;
    let lua = Lua::new();
    let value = lua
        .load(&script)
        .set_name(script_path.to_string_lossy().as_ref())
        .eval::<Value>()
        .map_err(|err| format!("Lua error: {}", err))?;

    if let Value::Table(table) = value {
        return Ok(Some(read_widget_table(table)?));
    }

    let globals = lua.globals();
    if let Some(table) = globals
        .get::<_, Option<Table>>("widget")
        .map_err(|err| err.to_string())?
    {
        return Ok(Some(read_widget_table(table)?));
    }

    let title: Option<String> = globals.get("title").map_err(|err| err.to_string())?;
    let lines_table: Option<Table> = globals.get("lines").map_err(|err| err.to_string())?;
    if let (Some(title), Some(lines_table)) = (title, lines_table) {
        let lines = read_lines_table(lines_table)?;
        return Ok(Some(WidgetData { title, lines }));
    }

    Err("Lua widget must return a table with `title` and `lines`".to_string())
}

fn read_widget_table(table: Table) -> Result<WidgetData, String> {
    let title: String = table
        .get("title")
        .map_err(|_| "Lua widget missing `title`".to_string())?;
    let lines_table: Table = table
        .get("lines")
        .map_err(|_| "Lua widget missing `lines`".to_string())?;
    let lines = read_lines_table(lines_table)?;
    Ok(WidgetData { title, lines })
}

fn read_lines_table(table: Table) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for pair in table.sequence_values::<String>() {
        let line = pair.map_err(|err| err.to_string())?;
        lines.push(line);
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_dir(label: &str) -> PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("omakure_lua_widget_{label}_{pid}_{nanos}"))
    }

    #[test]
    fn missing_index_lua_returns_none_without_error() {
        let dir = unique_dir("missing");
        std::fs::create_dir_all(&dir).expect("create dir");
        let result = load_widget(&dir).expect("absent index.lua must not error");
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn returns_widget_data_from_table_return() {
        let dir = unique_dir("return_table");
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::write(
            dir.join("index.lua"),
            "return { title = \"Team\", lines = { \"alpha\", \"beta\" } }",
        )
        .expect("write index.lua");

        let widget = load_widget(&dir)
            .expect("widget should load")
            .expect("widget should be present");
        assert_eq!(widget.title, "Team");
        assert_eq!(widget.lines, vec!["alpha".to_string(), "beta".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
