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

fn read_theme_names(theme_dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
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
