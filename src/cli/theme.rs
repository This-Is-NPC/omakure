use crate::adapters::omarchy;
use crate::adapters::tui::theme::{
    builtin_theme_names, load_theme_from_builtin, load_theme_from_name, theme_file_path, Theme,
    ThemeVariant,
};
use crate::cli::args::{ThemeArgs, ThemeCommand};
use crate::theme_config;
use ratatui::style::Color;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(_scripts_dir: PathBuf, args: ThemeArgs) -> Result<(), Box<dyn Error>> {
    match args.command {
        ThemeCommand::List => list_themes(),
        ThemeCommand::Set(args) => set_theme(&args.name),
        ThemeCommand::Preview(args) => preview_theme(&args.name),
        ThemeCommand::Path => print_paths(),
    }
}

fn list_themes() -> Result<(), Box<dyn Error>> {
    let layout = theme_config::ensure_theme_layout()?;
    let mut builtin = builtin_theme_names();
    builtin.sort();
    println!("Built-in themes:");
    for name in builtin {
        println!(" - {}", name);
    }

    let theme_dir = layout.themes_dir;
    let user_themes = if theme_dir.is_dir() {
        read_theme_names(&theme_dir)?
    } else {
        Vec::new()
    };

    println!("\nUser themes ({})", theme_dir.display());
    if user_themes.is_empty() {
        println!(" - (none)");
    } else {
        for name in user_themes {
            println!(" - {}", name);
        }
    }

    if omarchy::is_omarchy_system() {
        println!("\nOmarchy themes:");
        let current = omarchy::current_theme_name().unwrap_or_else(|| "unknown".to_string());
        println!(" - system (current: {})", omarchy::display_name(&current));
        for name in omarchy::list_themes() {
            println!(" - {}", name);
        }
    }

    Ok(())
}

fn set_theme(name: &str) -> Result<(), Box<dyn Error>> {
    let layout = theme_config::ensure_theme_layout()?;
    ensure_theme_exists(name, &layout.themes_dir)?;
    theme_config::write_global_theme(&layout.config_path, name)?;

    println!(
        "Theme set to '{}' in {}",
        name,
        layout.config_path.display()
    );
    Ok(())
}

fn preview_theme(name: &str) -> Result<(), Box<dyn Error>> {
    let layout = theme_config::ensure_theme_layout()?;
    let theme = if name == "system" {
        omarchy::resolve_system_colors().and_then(|colors| omarchy::map_to_theme("system", &colors))
    } else if let Some(theme) = load_theme_from_name(name, &layout.themes_dir) {
        Some(theme)
    } else if let Some(theme) = load_theme_from_builtin(name) {
        Some(theme)
    } else {
        omarchy::resolve_theme_colors(name).and_then(|colors| omarchy::map_to_theme(name, &colors))
    };

    match theme {
        Some(theme) => {
            print_theme_preview(name, &theme);
            Ok(())
        }
        None => Err(format!("Theme not found: {}", name).into()),
    }
}

fn print_paths() -> Result<(), Box<dyn Error>> {
    let layout = theme_config::ensure_theme_layout()?;
    println!("Config dir: {}", layout.config_dir.display());
    println!("Themes dir: {}", layout.themes_dir.display());
    println!("Config file: {}", layout.config_path.display());
    Ok(())
}

fn print_theme_preview(name: &str, theme: &Theme) {
    println!("Theme: {} ({})", theme.meta.name, name);
    if let Some(author) = theme.meta.author.as_deref() {
        println!("Author: {}", author);
    }
    if let Some(variant) = theme.meta.variant {
        println!("Variant: {}", format_variant(variant));
    }

    println!(
        "Brand: {} -> {}",
        format_color(theme.brand.gradient_start.color()),
        format_color(theme.brand.gradient_end.color())
    );
    println!("Accent: {}", format_color(theme.brand.accent.color()));
    println!(
        "Semantic: success {}, error {}, warning {}, info {}",
        format_color(theme.semantic.success.color()),
        format_color(theme.semantic.error.color()),
        format_color(theme.semantic.warning.color()),
        format_color(theme.semantic.info.color())
    );
    println!(
        "UI text: primary {}, secondary {}, muted {}",
        format_color(theme.ui.text_primary.color()),
        format_color(theme.ui.text_secondary.color()),
        format_color(theme.ui.text_muted.color())
    );
    println!(
        "UI borders: active {}, inactive {}",
        format_color(theme.ui.border_active.color()),
        format_color(theme.ui.border_inactive.color())
    );
    println!("Selection: {}", format_color(theme.ui.selection_fg.color()));
    println!(
        "Status: ok {}, fail {}, error {}",
        format_color(theme.status.ok.color()),
        format_color(theme.status.fail.color()),
        format_color(theme.status.error.color())
    );
}

fn format_variant(variant: ThemeVariant) -> &'static str {
    match variant {
        ThemeVariant::Dark => "dark",
        ThemeVariant::Light => "light",
    }
}

fn format_color(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{:02x}{:02x}{:02x}", r, g, b),
        _ => format!("{:?}", color),
    }
}

fn ensure_theme_exists(name: &str, theme_dir: &Path) -> Result<(), Box<dyn Error>> {
    if name == "system" {
        return Ok(());
    }

    let is_builtin = builtin_theme_names().contains(&name);
    if is_builtin {
        return Ok(());
    }

    let theme_path = theme_file_path(theme_dir, name);
    if theme_path.is_file() {
        return Ok(());
    }

    if omarchy::resolve_theme_colors(name).is_some() {
        return Ok(());
    }

    Err(format!("Theme not found: {}", name).into())
}

pub(crate) fn read_theme_names(theme_dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(theme_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            names.push(stem.to_string());
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::tui::theme::builtin_theme_names;
    use pretty_assertions::assert_eq;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_builtin_themes_count() {
        let names = builtin_theme_names();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"default"));
        assert!(names.contains(&"dracula"));
        assert!(names.contains(&"catppuccin-mocha"));
        assert!(names.contains(&"nord"));
        assert!(names.contains(&"solarized-dark"));
    }

    #[test]
    fn test_ensure_theme_exists_builtin() {
        let tmp = TempDir::new().unwrap();
        assert!(ensure_theme_exists("default", tmp.path()).is_ok());
        assert!(ensure_theme_exists("dracula", tmp.path()).is_ok());
    }

    #[test]
    fn test_ensure_theme_exists_not_found() {
        let tmp = TempDir::new().unwrap();
        let result = ensure_theme_exists("nonexistent_theme_xyz", tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_ensure_theme_exists_system() {
        let tmp = TempDir::new().unwrap();
        assert!(ensure_theme_exists("system", tmp.path()).is_ok());
    }

    #[test]
    fn test_read_theme_names_from_dir() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("custom.toml"), "[meta]\nname = \"custom\"").unwrap();
        fs::write(tmp.path().join("other.toml"), "[meta]\nname = \"other\"").unwrap();
        fs::write(tmp.path().join("readme.md"), "not a theme").unwrap();

        let names = read_theme_names(tmp.path()).unwrap();
        assert_eq!(names, vec!["custom", "other"]);
    }

    #[test]
    fn test_read_theme_names_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let names = read_theme_names(tmp.path()).unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn test_format_variant() {
        assert_eq!(format_variant(ThemeVariant::Dark), "dark");
        assert_eq!(format_variant(ThemeVariant::Light), "light");
    }

    #[test]
    fn test_format_color_rgb() {
        let result = format_color(Color::Rgb(255, 0, 128));
        assert_eq!(result, "#ff0080");
    }

    #[test]
    fn test_format_color_named_falls_back_to_debug() {
        let result = format_color(Color::Red);
        assert!(result.contains("Red"));
    }

    #[test]
    fn test_print_theme_preview_for_default_theme() {
        let theme = Theme::default();
        // Just exercise the print function — assertion is no panic.
        print_theme_preview("default", &theme);
    }

    #[test]
    fn test_ensure_theme_exists_user_theme_file() {
        let tmp = TempDir::new().unwrap();
        let path = theme_file_path(tmp.path(), "custom");
        fs::write(&path, "[meta]\nname = \"custom\"").unwrap();
        assert!(ensure_theme_exists("custom", tmp.path()).is_ok());
    }

    #[test]
    fn test_run_dispatches_to_subcommands() {
        // Point XDG_CONFIG_HOME at a temp dir so we don't write to the
        // user's real config.
        let tmp = TempDir::new().unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());

        let scripts_dir = tmp.path().to_path_buf();
        let _ = run(
            scripts_dir.clone(),
            ThemeArgs {
                command: ThemeCommand::List,
            },
        );
        let _ = run(
            scripts_dir.clone(),
            ThemeArgs {
                command: ThemeCommand::Path,
            },
        );
        let _ = run(
            scripts_dir.clone(),
            ThemeArgs {
                command: ThemeCommand::Set(crate::cli::args::ThemeSetArgs {
                    name: "default".into(),
                }),
            },
        );
        let _ = run(
            scripts_dir.clone(),
            ThemeArgs {
                command: ThemeCommand::Preview(crate::cli::args::ThemeSetArgs {
                    name: "default".into(),
                }),
            },
        );
        // Unknown theme returns an error path.
        let err = run(
            scripts_dir,
            ThemeArgs {
                command: ThemeCommand::Preview(crate::cli::args::ThemeSetArgs {
                    name: "__no_such_theme__".into(),
                }),
            },
        );
        assert!(err.is_err());

        match prev {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}
