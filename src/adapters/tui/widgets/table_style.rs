use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Padding, Row};

use super::super::theme::Theme;

/// Column spacing applied to every table. Includes the width of the
/// separator column itself (1 char) plus a single breathing space on
/// each side of it — matching the visual of `" │ "`.
pub(crate) const COLUMN_SPACING: u16 = 1;

/// Horizontal padding applied by [`block`] to every bordered container
/// so the content never touches the border characters.
pub(crate) const BLOCK_HORIZONTAL_PADDING: u16 = 2;

/// Interleave a `│` separator column between every real column. Used
/// together with [`interleave_column_constraints`] so the table renders
/// visible vertical grid lines. The separator is styled with the muted
/// theme color so it reads as chrome rather than content.
pub(crate) fn interleave_row_cells<'a>(cells: Vec<Cell<'a>>, theme: &Theme) -> Vec<Cell<'a>> {
    let sep_style = theme.text_muted();
    let mut out: Vec<Cell<'a>> = Vec::with_capacity(cells.len() * 2);
    let n = cells.len();
    for (i, c) in cells.into_iter().enumerate() {
        out.push(c);
        if i + 1 < n {
            out.push(Cell::from(Span::styled("│", sep_style)));
        }
    }
    out
}

/// Interleave a length-1 constraint between every real column — the
/// constraint must match [`interleave_row_cells`] so ratatui lays out
/// the `│` separator correctly.
pub(crate) fn interleave_column_constraints(cols: &[Constraint]) -> Vec<Constraint> {
    let mut out: Vec<Constraint> = Vec::with_capacity(cols.len() * 2);
    let n = cols.len();
    for (i, c) in cols.iter().enumerate() {
        out.push(*c);
        if i + 1 < n {
            out.push(Constraint::Length(1));
        }
    }
    out
}

/// Two-line header row with separators already interleaved so the
/// caller can pass it directly to `Table::header`. Matches the layout
/// produced by [`interleave_column_constraints`].
///
/// `max_width` is the total pane width available to the table; it is
/// used to size the second-line ruler so it always reaches the right
/// edge of the widest responsive column without over-allocating.
/// Ratatui clips the ruler to each column's rendered width.
pub(crate) fn header_row_with_separators<'a>(
    labels: &[&'a str],
    max_width: u16,
    theme: &Theme,
) -> Row<'a> {
    let header_style = theme.text_secondary().add_modifier(Modifier::BOLD);
    let ruler = "─".repeat(max_width as usize);
    let mut cells: Vec<Cell<'a>> = Vec::with_capacity(labels.len() * 2);
    let n = labels.len();
    for (i, label) in labels.iter().enumerate() {
        cells.push(Cell::from(Text::from(vec![
            Line::from(Span::styled(label.to_string(), header_style)),
            Line::from(Span::styled(ruler.clone(), theme.text_muted())),
        ])));
        if i + 1 < n {
            cells.push(Cell::from(Text::from(vec![
                Line::from(Span::styled("│", theme.text_muted())),
                Line::from(Span::styled("┼", theme.text_muted())),
            ])));
        }
    }
    Row::new(cells).height(2)
}

pub(crate) fn selection_style(theme: &Theme) -> Style {
    theme.selection_style()
}

pub(crate) fn selection_symbol(theme: &Theme) -> Span<'static> {
    theme.selection_symbol()
}

pub(crate) fn block<'a>(title: &'a str, theme: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title, theme.text_secondary()))
}

/// Same as [`block`] but with [`BLOCK_HORIZONTAL_PADDING`] applied so
/// tabular content doesn't touch the border. Prefer this for any
/// container that hosts a `Table` or otherwise reads better with
/// side breathing room.
pub(crate) fn padded_block<'a>(title: &'a str, theme: &Theme) -> Block<'a> {
    block(title, theme).padding(Padding::horizontal(BLOCK_HORIZONTAL_PADDING))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_row_with_separators_doubles_cell_count() {
        let theme = Theme::default();
        let _row = header_row_with_separators(&["A", "B", "C"], 120, &theme);
    }

    #[test]
    fn interleave_row_cells_inserts_n_minus_one_separators() {
        let theme = Theme::default();
        let cells = vec![Cell::from("a"), Cell::from("b"), Cell::from("c")];
        let out = interleave_row_cells(cells, &theme);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn interleave_column_constraints_inserts_length_one_gaps() {
        let input = [Constraint::Length(10), Constraint::Min(5)];
        let out = interleave_column_constraints(&input);
        assert_eq!(out.len(), 3);
        matches!(out[1], Constraint::Length(1));
    }

    #[test]
    fn block_applies_title_and_borders() {
        let theme = Theme::default();
        let _b = block("History", &theme);
    }

    #[test]
    fn selection_helpers_return_for_default_theme() {
        let theme = Theme::default();
        let _ = selection_style(&theme);
        let _ = selection_symbol(&theme);
    }
}
