use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

pub(crate) const BRAND_GRADIENT_START: (u8, u8, u8) = (245, 170, 80);
pub(crate) const BRAND_GRADIENT_END: (u8, u8, u8) = (205, 85, 85);

pub(crate) fn brand_accent() -> Color {
    Color::Rgb(
        BRAND_GRADIENT_START.0,
        BRAND_GRADIENT_START.1,
        BRAND_GRADIENT_START.2,
    )
}

pub(crate) fn selection_style() -> Style {
    Style::default()
        .fg(brand_accent())
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn selection_border_style() -> Style {
    selection_style()
}

pub(crate) fn selection_symbol() -> Span<'static> {
    Span::styled("> ", selection_style())
}

pub(crate) fn selection_symbol_str() -> &'static str {
    "> "
}
