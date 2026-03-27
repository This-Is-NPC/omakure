use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use serde::Deserialize;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const BRAND_GRADIENT_START: (u8, u8, u8) = (245, 170, 80);
pub(crate) const BRAND_GRADIENT_END: (u8, u8, u8) = (205, 85, 85);

const DEFAULT_THEME_TOML: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/default.toml"));

#[derive(Debug, Clone, Copy)]
pub(crate) struct BuiltinTheme {
    pub name: &'static str,
    pub contents: &'static str,
}

pub(crate) const BUILTIN_THEMES: &[BuiltinTheme] = &[
    BuiltinTheme {
        name: "default",
        contents: DEFAULT_THEME_TOML,
    },
    BuiltinTheme {
        name: "dracula",
        contents: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/dracula.toml")),
    },
    BuiltinTheme {
        name: "catppuccin-mocha",
        contents: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/catppuccin-mocha.toml"
        )),
    },
    BuiltinTheme {
        name: "nord",
        contents: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/nord.toml")),
    },
    BuiltinTheme {
        name: "solarized-dark",
        contents: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/solarized-dark.toml"
        )),
    },
];

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Theme {
    pub meta: ThemeMeta,
    pub brand: BrandColors,
    pub semantic: SemanticColors,
    pub ui: UiColors,
    pub status: StatusColors,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ThemeMeta {
    pub name: String,
    pub author: Option<String>,
    pub variant: Option<ThemeVariant>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThemeVariant {
    Dark,
    Light,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BrandColors {
    pub gradient_start: HexColor,
    pub gradient_end: HexColor,
    pub accent: HexColor,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SemanticColors {
    pub success: HexColor,
    pub error: HexColor,
    pub warning: HexColor,
    pub info: HexColor,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UiColors {
    pub text_primary: HexColor,
    pub text_secondary: HexColor,
    pub text_muted: HexColor,
    pub border_active: HexColor,
    pub border_inactive: HexColor,
    pub selection_fg: HexColor,
    pub selection_bg: Option<HexColor>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct StatusColors {
    pub ok: HexColor,
    pub fail: HexColor,
    pub error: HexColor,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct HexColor(pub Color);

#[derive(Debug)]
pub(crate) enum ThemeParseError {
    InvalidHexLength(String),
    InvalidHexValue(String),
}

impl fmt::Display for ThemeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThemeParseError::InvalidHexLength(value) => {
                write!(f, "invalid hex length: {}", value)
            }
            ThemeParseError::InvalidHexValue(value) => write!(f, "invalid hex value: {}", value),
        }
    }
}

impl Error for ThemeParseError {}

#[derive(Debug)]
pub(crate) enum ThemeLoadError {
    Io(std::io::Error),
    Parse(toml::de::Error),
}

impl fmt::Display for ThemeLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThemeLoadError::Io(err) => write!(f, "theme io error: {}", err),
            ThemeLoadError::Parse(err) => write!(f, "theme parse error: {}", err),
        }
    }
}

impl Error for ThemeLoadError {}

impl From<std::io::Error> for ThemeLoadError {
    fn from(value: std::io::Error) -> Self {
        ThemeLoadError::Io(value)
    }
}

impl From<toml::de::Error> for ThemeLoadError {
    fn from(value: toml::de::Error) -> Self {
        ThemeLoadError::Parse(value)
    }
}

impl TryFrom<String> for HexColor {
    type Error = ThemeParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        parse_hex_color(&value).map(HexColor)
    }
}

impl HexColor {
    pub(crate) fn new(color: Color) -> Self {
        Self(color)
    }

    pub(crate) fn color(&self) -> Color {
        self.0.clone()
    }
}

impl Default for Theme {
    fn default() -> Self {
        default_theme()
    }
}

impl Theme {
    pub(crate) fn selection_style(&self) -> Style {
        let mut style = Style::default()
            .fg(self.ui.selection_fg.color())
            .add_modifier(Modifier::BOLD);
        if let Some(bg) = self.ui.selection_bg.as_ref() {
            style = style.bg(bg.color());
        }
        style
    }

    pub(crate) fn selection_border_style(&self) -> Style {
        self.selection_style()
    }

    pub(crate) fn selection_symbol(&self) -> Span<'static> {
        Span::styled("> ", self.selection_style())
    }

    pub(crate) fn text_secondary(&self) -> Style {
        Style::default().fg(self.ui.text_secondary.color())
    }

    pub(crate) fn text_muted(&self) -> Style {
        Style::default().fg(self.ui.text_muted.color())
    }

    pub(crate) fn status_ok_style(&self) -> Style {
        Style::default().fg(self.status.ok.color())
    }

    pub(crate) fn status_fail_style(&self) -> Style {
        Style::default().fg(self.status.fail.color())
    }

    pub(crate) fn status_error_style(&self) -> Style {
        Style::default().fg(self.status.error.color())
    }
}

pub(crate) fn default_theme() -> Theme {
    load_theme_from_str(DEFAULT_THEME_TOML).unwrap_or_else(|_| fallback_default_theme())
}

fn fallback_default_theme() -> Theme {
    Theme {
        meta: ThemeMeta {
            name: "Omakure Default".to_string(),
            author: None,
            variant: None,
        },
        brand: BrandColors {
            gradient_start: HexColor::new(color_from_tuple(BRAND_GRADIENT_START)),
            gradient_end: HexColor::new(color_from_tuple(BRAND_GRADIENT_END)),
            accent: HexColor::new(color_from_tuple(BRAND_GRADIENT_START)),
        },
        semantic: SemanticColors {
            success: HexColor::new(Color::Green),
            error: HexColor::new(Color::Red),
            warning: HexColor::new(Color::Yellow),
            info: HexColor::new(Color::Cyan),
        },
        ui: UiColors {
            text_primary: HexColor::new(Color::White),
            text_secondary: HexColor::new(Color::Gray),
            text_muted: HexColor::new(Color::DarkGray),
            border_active: HexColor::new(color_from_tuple(BRAND_GRADIENT_START)),
            border_inactive: HexColor::new(Color::Gray),
            selection_fg: HexColor::new(color_from_tuple(BRAND_GRADIENT_START)),
            selection_bg: None,
        },
        status: StatusColors {
            ok: HexColor::new(Color::Green),
            fail: HexColor::new(Color::Red),
            error: HexColor::new(Color::Yellow),
        },
    }
}

pub(crate) fn load_theme(theme_name: Option<&str>, theme_dir: Option<&Path>) -> Theme {
    if let Some(name) = theme_name {
        if name == "system" {
            if let Some(colors) = crate::adapters::omarchy::resolve_system_colors() {
                if let Some(theme) = crate::adapters::omarchy::map_to_theme("system", &colors) {
                    return theme;
                }
            }
            return default_theme();
        }
        if let Some(dir) = theme_dir {
            if let Some(theme) = load_theme_from_name(name, dir) {
                return theme;
            }
        }
        if let Some(theme) = load_theme_from_builtin(name) {
            return theme;
        }
        if let Some(colors) = crate::adapters::omarchy::resolve_theme_colors(name) {
            if let Some(theme) = crate::adapters::omarchy::map_to_theme(name, &colors) {
                return theme;
            }
        }
    }
    default_theme()
}

pub(crate) fn builtin_theme_names() -> Vec<&'static str> {
    BUILTIN_THEMES.iter().map(|theme| theme.name).collect()
}

pub(crate) fn builtin_theme_contents(name: &str) -> Option<&'static str> {
    BUILTIN_THEMES
        .iter()
        .find(|theme| theme.name == name)
        .map(|theme| theme.contents)
}

pub(crate) fn load_theme_from_builtin(name: &str) -> Option<Theme> {
    builtin_theme_contents(name).and_then(|contents| load_theme_from_str(contents).ok())
}

pub(crate) fn load_theme_from_name(name: &str, theme_dir: &Path) -> Option<Theme> {
    let file_name = format!("{}.toml", name);
    let path = theme_dir.join(file_name);
    load_theme_from_path(&path).ok()
}

pub(crate) fn load_theme_from_path(path: &Path) -> Result<Theme, ThemeLoadError> {
    let contents = fs::read_to_string(path)?;
    load_theme_from_str(&contents)
}

pub(crate) fn load_theme_from_str(contents: &str) -> Result<Theme, ThemeLoadError> {
    let theme = toml::from_str::<Theme>(contents)?;
    Ok(theme)
}

pub(crate) fn theme_file_path(theme_dir: &Path, theme_name: &str) -> PathBuf {
    theme_dir.join(format!("{}.toml", theme_name))
}

fn color_from_tuple(rgb: (u8, u8, u8)) -> Color {
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}

fn parse_hex_color(value: &str) -> Result<Color, ThemeParseError> {
    let trimmed = value.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    let normalized = match hex.len() {
        3 => {
            let mut expanded = String::with_capacity(6);
            for ch in hex.chars() {
                expanded.push(ch);
                expanded.push(ch);
            }
            expanded
        }
        6 => hex.to_string(),
        _ => return Err(ThemeParseError::InvalidHexLength(value.to_string())),
    };

    let red = u8::from_str_radix(&normalized[0..2], 16)
        .map_err(|_| ThemeParseError::InvalidHexValue(value.to_string()))?;
    let green = u8::from_str_radix(&normalized[2..4], 16)
        .map_err(|_| ThemeParseError::InvalidHexValue(value.to_string()))?;
    let blue = u8::from_str_radix(&normalized[4..6], 16)
        .map_err(|_| ThemeParseError::InvalidHexValue(value.to_string()))?;
    Ok(Color::Rgb(red, green, blue))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_theme_from_str_parses_colors() {
        let toml = r##"
[meta]
name = "Test Theme"
variant = "dark"

[brand]
gradient_start = "#000000"
gradient_end = "#ffffff"
accent = "#ff0000"

[semantic]
success = "#00ff00"
error = "#ff0000"
warning = "#ffff00"
info = "#00ffff"

[ui]
text_primary = "#ffffff"
text_secondary = "#aaaaaa"
text_muted = "#555555"
border_active = "#123456"
border_inactive = "#654321"
selection_fg = "#abcdef"

[status]
ok = "#00ff00"
fail = "#ff0000"
error = "#ffff00"
"##;
        let theme = load_theme_from_str(toml).expect("theme should parse");
        assert_eq!(theme.meta.name, "Test Theme");
        assert_eq!(theme.brand.accent.color(), Color::Rgb(255, 0, 0));
    }

    #[test]
    fn load_theme_falls_back_to_default() {
        let theme = load_theme(Some("missing-theme"), None);
        assert_eq!(theme.meta.name, "Omakure Default");
    }

    #[test]
    fn load_theme_from_str_rejects_invalid_hex() {
        let toml = r##"
[meta]
name = "Broken Theme"

[brand]
gradient_start = "#zzzzzz"
gradient_end = "#ffffff"
accent = "#ff0000"

[semantic]
success = "#00ff00"
error = "#ff0000"
warning = "#ffff00"
info = "#00ffff"

[ui]
text_primary = "#ffffff"
text_secondary = "#aaaaaa"
text_muted = "#555555"
border_active = "#123456"
border_inactive = "#654321"
selection_fg = "#abcdef"

[status]
ok = "#00ff00"
fail = "#ff0000"
error = "#ffff00"
"##;
        assert!(load_theme_from_str(toml).is_err());
    }
}

pub(crate) fn selection_symbol_str() -> &'static str {
    "> "
}
