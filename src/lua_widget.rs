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
    if let Some(table) = globals.get::<_, Option<Table>>("widget").map_err(|err| err.to_string())?
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
