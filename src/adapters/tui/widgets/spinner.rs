//! Themed loading spinners.
//!
//! Thin wrapper over the `rattles` crate that exposes one helper per
//! visual theme. Each helper takes a `tick` counter (provided by
//! `App.tick`, incremented once per main-loop iteration in
//! `src/adapters/tui/mod.rs`) and returns a ratatui [`Span`] colored
//! by the active theme. No internal state, no thread, no clock — the
//! same `tick` value always produces the same glyph.

use ratatui::text::Span;
use rattles::presets::braille::{Sand, Scan};
use rattles::Rattle;

use super::super::theme::Theme;

/// Visual theme of a loading spinner. The variant is chosen by the
/// site that needs the spinner; only `Scan` is currently used for
/// search-style operations, the rest of the app falls back to `Sand`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum SpinnerKind {
    Scan,
    Sand,
}

/// Pick the next braille glyph for the given spinner kind, indexed
/// purely by `tick % frames.len()` so animation is deterministic and
/// driven entirely by the main TUI loop. Returns a single grapheme
/// (or short multi-grapheme run for `Scan`, which has width 4).
pub(crate) fn spinner_glyph(kind: SpinnerKind, tick: u64) -> &'static str {
    match kind {
        SpinnerKind::Scan => pick_frame::<Scan>(tick),
        SpinnerKind::Sand => pick_frame::<Sand>(tick),
    }
}

fn pick_frame<R: Rattle>(tick: u64) -> &'static str {
    let frames = R::FRAMES;
    if frames.is_empty() {
        return "";
    }
    let idx = (tick as usize) % frames.len();
    let frame = frames[idx];
    if frame.is_empty() {
        ""
    } else {
        frame[0]
    }
}

/// Themed convenience: returns a [`Span`] containing the next glyph
/// followed by a single space, styled with `theme.text_secondary()` so
/// the spinner color tracks the active theme everywhere it is used.
pub(crate) fn spinner_span(kind: SpinnerKind, tick: u64, theme: &Theme) -> Span<'static> {
    let glyph = spinner_glyph(kind, tick);
    Span::styled(format!("{} ", glyph), theme.text_secondary())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_is_stable_for_same_tick() {
        let a = spinner_glyph(SpinnerKind::Sand, 7);
        let b = spinner_glyph(SpinnerKind::Sand, 7);
        assert_eq!(a, b);
    }

    #[test]
    fn glyph_cycles_back_to_first_after_full_cycle() {
        let len = Sand::FRAMES.len() as u64;
        assert!(len > 0);
        let first = spinner_glyph(SpinnerKind::Sand, 0);
        let after_one_cycle = spinner_glyph(SpinnerKind::Sand, len);
        assert_eq!(first, after_one_cycle);
    }

    #[test]
    fn scan_and_sand_produce_distinct_sequences() {
        // Pick a tick where the two sequences are guaranteed to differ —
        // index 0 of Sand is "⠁" while Scan's first frame is empty.
        let scan = spinner_glyph(SpinnerKind::Scan, 0);
        let sand = spinner_glyph(SpinnerKind::Sand, 0);
        assert_ne!(scan, sand);
    }

    #[test]
    fn glyph_handles_overflow_via_modulo() {
        // u64::MAX % len should still produce a valid frame string.
        let frame = spinner_glyph(SpinnerKind::Sand, u64::MAX);
        assert!(!frame.is_empty());
    }

    #[test]
    fn spinner_span_renders_glyph_with_trailing_space() {
        let theme = Theme::default();
        let span = spinner_span(SpinnerKind::Sand, 0, &theme);
        assert!(span.content.ends_with(' '));
    }
}
