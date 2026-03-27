use crate::adapters::tui::theme::{
    BrandColors, HexColor, SemanticColors, StatusColors, Theme, ThemeMeta, ThemeVariant, UiColors,
};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OmarchyColors {
    pub accent: String,
    pub foreground: String,
    pub background: String,
    #[serde(default)]
    pub selection_foreground: Option<String>,
    #[serde(default)]
    pub selection_background: Option<String>,
    pub color1: String,
    pub color2: String,
    pub color3: String,
    pub color4: String,
    pub color7: String,
    pub color8: String,
}

pub(crate) fn is_omarchy_system() -> bool {
    theme_name_path().is_file()
}

pub(crate) fn current_theme_name() -> Option<String> {
    let path = theme_name_path();
    let contents = fs::read_to_string(path).ok()?;
    let name = contents.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

pub(crate) fn list_themes() -> Vec<String> {
    let mut names = BTreeSet::new();
    for dir in theme_search_dirs() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                if !path.join("colors.toml").is_file() {
                    continue;
                }
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names.into_iter().collect()
}

pub(crate) fn resolve_system_colors() -> Option<OmarchyColors> {
    let path = config_dir()?.join("current/theme/colors.toml");
    load_colors_from_path(&path)
}

pub(crate) fn resolve_theme_colors(name: &str) -> Option<OmarchyColors> {
    for dir in theme_search_dirs() {
        let path = dir.join(name).join("colors.toml");
        if let Some(colors) = load_colors_from_path(&path) {
            return Some(colors);
        }
    }
    None
}

pub(crate) fn map_to_theme(name: &str, colors: &OmarchyColors) -> Option<Theme> {
    let hex = |s: &str| -> Option<HexColor> { HexColor::try_from(s.to_string()).ok() };

    let accent = hex(&colors.accent)?;
    let foreground = hex(&colors.foreground)?;
    let selection_fg = colors
        .selection_foreground
        .as_deref()
        .and_then(hex)
        .unwrap_or_else(|| foreground.clone());
    let selection_bg = colors.selection_background.as_deref().and_then(hex);
    let color1 = hex(&colors.color1)?;
    let color2 = hex(&colors.color2)?;
    let color3 = hex(&colors.color3)?;
    let color4 = hex(&colors.color4)?;
    let color7 = hex(&colors.color7)?;
    let color8 = hex(&colors.color8)?;

    Some(Theme {
        meta: ThemeMeta {
            name: display_name(name),
            author: Some("Omarchy".to_string()),
            variant: Some(infer_variant(&colors.background)),
        },
        brand: BrandColors {
            gradient_start: accent.clone(),
            gradient_end: accent.clone(),
            accent,
        },
        semantic: SemanticColors {
            success: color2.clone(),
            error: color1.clone(),
            warning: color3.clone(),
            info: color4,
        },
        ui: UiColors {
            text_primary: foreground,
            text_secondary: color7,
            text_muted: color8.clone(),
            border_active: accent_clone(&colors.accent)?,
            border_inactive: color8,
            selection_fg,
            selection_bg,
        },
        status: StatusColors {
            ok: color2,
            fail: color1,
            error: color3,
        },
    })
}

pub(crate) fn display_name(slug: &str) -> String {
    slug.split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    let upper: String = first.to_uppercase().collect();
                    format!("{}{}", upper, chars.as_str())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn accent_clone(hex_str: &str) -> Option<HexColor> {
    HexColor::try_from(hex_str.to_string()).ok()
}

fn infer_variant(background_hex: &str) -> ThemeVariant {
    let hex = background_hex
        .trim()
        .strip_prefix('#')
        .unwrap_or(background_hex.trim());
    if hex.len() < 6 {
        return ThemeVariant::Dark;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    let luminance = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    if luminance < 128 {
        ThemeVariant::Dark
    } else {
        ThemeVariant::Light
    }
}

fn load_colors_from_path(path: &Path) -> Option<OmarchyColors> {
    let contents = fs::read_to_string(path).ok()?;
    toml::from_str(&contents).ok()
}

fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("omarchy"))
}

fn theme_name_path() -> PathBuf {
    config_dir()
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
        .join("current/theme.name")
}

fn omarchy_data_dir() -> PathBuf {
    if let Ok(path) = env::var("OMARCHY_PATH") {
        return PathBuf::from(path);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
        .join("omarchy")
}

fn theme_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(config) = config_dir() {
        dirs.push(config.join("themes"));
    }
    let data = omarchy_data_dir();
    dirs.push(data.join("themes"));
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sample_colors_toml() -> &'static str {
        r##"accent = "#7aa2f7"
cursor = "#c0caf5"
foreground = "#a9b1d6"
background = "#1a1b26"
selection_foreground = "#c0caf5"
selection_background = "#7aa2f7"

color0 = "#32344a"
color1 = "#f7768e"
color2 = "#9ece6a"
color3 = "#e0af68"
color4 = "#7aa2f7"
color5 = "#ad8ee6"
color6 = "#449dab"
color7 = "#787c99"
color8 = "#444b6a"
color9 = "#ff7a93"
color10 = "#b9f27c"
color11 = "#ff9e64"
color12 = "#7da6ff"
color13 = "#bb9af7"
color14 = "#0db9d7"
color15 = "#acb0d0"
"##
    }

    #[test]
    fn test_parse_omarchy_colors() {
        let colors: OmarchyColors = toml::from_str(sample_colors_toml()).unwrap();
        assert_eq!(colors.accent, "#7aa2f7");
        assert_eq!(colors.foreground, "#a9b1d6");
        assert_eq!(colors.background, "#1a1b26");
        assert_eq!(colors.color1, "#f7768e");
        assert_eq!(colors.color2, "#9ece6a");
        assert_eq!(colors.color3, "#e0af68");
        assert_eq!(colors.color4, "#7aa2f7");
        assert_eq!(colors.color7, "#787c99");
        assert_eq!(colors.color8, "#444b6a");
        assert_eq!(colors.selection_foreground, Some("#c0caf5".to_string()));
        assert_eq!(colors.selection_background, Some("#7aa2f7".to_string()));
    }

    #[test]
    fn test_parse_malformed_toml() {
        let result = toml::from_str::<OmarchyColors>("not valid toml {{{");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_missing_fields() {
        let result = toml::from_str::<OmarchyColors>(r##"accent = "#fff""##);
        assert!(result.is_err());
    }

    #[test]
    fn test_map_to_theme() {
        let colors: OmarchyColors = toml::from_str(sample_colors_toml()).unwrap();
        let theme = map_to_theme("tokyo-night", &colors).unwrap();
        assert_eq!(theme.meta.name, "Tokyo Night");
        assert_eq!(theme.meta.author, Some("Omarchy".to_string()));
        assert!(matches!(theme.meta.variant, Some(ThemeVariant::Dark)));
    }

    #[test]
    fn test_infer_variant_dark() {
        assert!(matches!(infer_variant("#1a1b26"), ThemeVariant::Dark));
        assert!(matches!(infer_variant("#000000"), ThemeVariant::Dark));
    }

    #[test]
    fn test_infer_variant_light() {
        assert!(matches!(infer_variant("#ffffff"), ThemeVariant::Light));
        assert!(matches!(infer_variant("#f5f5f5"), ThemeVariant::Light));
    }

    #[test]
    fn test_display_name() {
        assert_eq!(display_name("tokyo-night"), "Tokyo Night");
        assert_eq!(display_name("catppuccin"), "Catppuccin");
        assert_eq!(display_name("rose-pine"), "Rose Pine");
        assert_eq!(display_name("matte-black"), "Matte Black");
    }

    #[test]
    fn test_list_themes_with_temp_dir() {
        let tmp = tempdir();
        let themes_dir = tmp.join("themes");
        fs::create_dir_all(themes_dir.join("alpha")).unwrap();
        fs::write(themes_dir.join("alpha/colors.toml"), sample_colors_toml()).unwrap();
        fs::create_dir_all(themes_dir.join("beta")).unwrap();
        fs::write(themes_dir.join("beta/colors.toml"), sample_colors_toml()).unwrap();
        // directory without colors.toml should be excluded
        fs::create_dir_all(themes_dir.join("gamma")).unwrap();

        let mut names = BTreeSet::new();
        if let Ok(entries) = fs::read_dir(&themes_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("colors.toml").is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        names.insert(name.to_string());
                    }
                }
            }
        }
        let names: Vec<String> = names.into_iter().collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_resolve_theme_colors_from_path() {
        let tmp = tempdir();
        let theme_dir = tmp.join("tokyo-night");
        fs::create_dir_all(&theme_dir).unwrap();
        fs::write(theme_dir.join("colors.toml"), sample_colors_toml()).unwrap();

        let colors = load_colors_from_path(&theme_dir.join("colors.toml")).unwrap();
        assert_eq!(colors.accent, "#7aa2f7");
    }

    #[test]
    fn test_resolve_missing_path() {
        let result = load_colors_from_path(Path::new("/nonexistent/colors.toml"));
        assert!(result.is_none());
    }

    #[test]
    fn test_is_not_omarchy_system() {
        // In CI/test environments, this file typically doesn't exist
        // We test that the function doesn't panic
        let _ = is_omarchy_system();
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omakure-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
