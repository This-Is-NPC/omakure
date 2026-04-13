use crate::adapters::tui::theme::{theme_file_path, BUILTIN_THEMES};
use serde::Deserialize;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct ThemeLayout {
    pub config_dir: PathBuf,
    pub themes_dir: PathBuf,
    pub config_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct ThemeConfigFile {
    theme: Option<ThemeConfig>,
}

#[derive(Debug, Deserialize)]
struct ThemeConfig {
    name: Option<String>,
}

pub(crate) fn ensure_theme_layout() -> Result<ThemeLayout, Box<dyn Error>> {
    let Some(config_dir) = config_dir() else {
        return Err("Unable to resolve config directory".into());
    };
    let themes_dir = config_dir.join("themes");
    let config_path = config_dir.join("config.toml");

    fs::create_dir_all(&themes_dir)?;
    ensure_builtin_themes(&themes_dir)?;

    if !config_path.exists() {
        write_global_theme(&config_path, "default")?;
    }

    Ok(ThemeLayout {
        config_dir,
        themes_dir,
        config_path,
    })
}

pub(crate) fn load_theme_name(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let config: ThemeConfigFile = toml::from_str(&contents).ok()?;
    config.theme.and_then(|theme| theme.name)
}

pub(crate) fn write_global_theme(path: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    let mut value = if path.exists() {
        let contents = fs::read_to_string(path)?;
        toml::from_str::<toml::Value>(&contents)?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let table = value
        .as_table_mut()
        .ok_or_else(|| "Config root is not a table".to_string())?;
    let theme_value = table
        .entry("theme".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    match theme_value {
        toml::Value::Table(theme_table) => {
            theme_table.insert("name".to_string(), toml::Value::String(name.to_string()));
        }
        _ => {
            let mut theme_table = toml::value::Table::new();
            theme_table.insert("name".to_string(), toml::Value::String(name.to_string()));
            *theme_value = toml::Value::Table(theme_table);
        }
    }

    let output = toml::to_string_pretty(&value)?;
    fs::write(path, output)?;
    Ok(())
}

fn ensure_builtin_themes(themes_dir: &Path) -> Result<(), Box<dyn Error>> {
    for theme in BUILTIN_THEMES {
        let path = theme_file_path(themes_dir, theme.name);
        if path.exists() {
            continue;
        }
        fs::write(path, theme.contents)?;
    }
    Ok(())
}

fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("omakure"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_theme_name_valid() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");
        fs::write(&config, "[theme]\nname = \"dracula\"\n").unwrap();

        let name = load_theme_name(&config);
        assert_eq!(name, Some("dracula".to_string()));
    }

    #[test]
    fn test_load_theme_name_no_file() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("nonexistent.toml");

        let name = load_theme_name(&config);
        assert_eq!(name, None);
    }

    #[test]
    fn test_load_theme_name_no_theme_section() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");
        fs::write(&config, "[other]\nkey = \"value\"\n").unwrap();

        let name = load_theme_name(&config);
        assert_eq!(name, None);
    }

    #[test]
    fn test_load_theme_name_invalid_toml() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");
        fs::write(&config, "not valid toml {{{{").unwrap();

        let name = load_theme_name(&config);
        assert_eq!(name, None);
    }

    #[test]
    fn test_write_and_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");

        write_global_theme(&config, "nord").unwrap();
        let name = load_theme_name(&config);
        assert_eq!(name, Some("nord".to_string()));
    }

    #[test]
    fn test_write_preserves_other_keys() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");
        fs::write(&config, "[other]\nkey = \"value\"\n").unwrap();

        write_global_theme(&config, "catppuccin-mocha").unwrap();

        let contents = fs::read_to_string(&config).unwrap();
        assert!(contents.contains("key = \"value\""));

        let name = load_theme_name(&config);
        assert_eq!(name, Some("catppuccin-mocha".to_string()));
    }

    #[test]
    fn test_write_overwrites_existing_theme() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");

        write_global_theme(&config, "dracula").unwrap();
        write_global_theme(&config, "nord").unwrap();

        let name = load_theme_name(&config);
        assert_eq!(name, Some("nord".to_string()));
    }

    #[test]
    fn test_write_replaces_scalar_theme_value() {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");
        fs::write(&config, "theme = \"legacy\"\n").unwrap();

        write_global_theme(&config, "nord").unwrap();

        let name = load_theme_name(&config);
        assert_eq!(name, Some("nord".to_string()));
    }

    #[test]
    fn test_ensure_builtin_themes_creates_files() {
        let tmp = TempDir::new().unwrap();
        ensure_builtin_themes(tmp.path()).unwrap();

        for theme in BUILTIN_THEMES {
            let path = theme_file_path(tmp.path(), theme.name);
            assert!(path.exists(), "missing builtin theme: {}", theme.name);
        }
    }

    #[test]
    fn test_ensure_builtin_themes_skips_existing() {
        let tmp = TempDir::new().unwrap();
        let first = BUILTIN_THEMES.first().expect("at least one builtin theme");
        let path = theme_file_path(tmp.path(), first.name);
        fs::write(&path, "custom").unwrap();

        ensure_builtin_themes(tmp.path()).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "custom");
    }

    #[test]
    fn test_config_dir_returns_some() {
        assert!(config_dir().is_some());
    }

    #[test]
    fn test_ensure_theme_layout_creates_paths() {
        let tmp = TempDir::new().unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());

        let layout = ensure_theme_layout().unwrap();
        assert!(layout.config_dir.exists());
        assert!(layout.themes_dir.exists());
        assert!(layout.config_path.exists());

        match prev {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}
