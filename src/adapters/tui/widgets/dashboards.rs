//! In-memory aggregations and rendering for the History `Dashboards` view.
//!
//! All aggregation functions in this module are pure: they take a slice of
//! [`RunRow`] and produce plain-data structs that the rendering helpers
//! consume. This keeps the math fully unit-testable without spinning up a
//! terminal — the same pattern used by `widgets/history.rs::tests`.

use std::path::PathBuf;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Sparkline, Wrap};
use ratatui::Frame;

use super::super::app::{App, DashboardLayout};
use super::super::theme::Theme;
use super::common::state_color;
use crate::runs::{RunRow, RunState};

/// Default number of recent runs displayed in the per-script duration
/// sparkline.
pub(crate) const PER_SCRIPT_DURATION_WINDOW: usize = 20;

/// Default number of scripts shown in the global "Top runs" panel.
pub(crate) const TOP_SCRIPTS_DEFAULT: usize = 6;

/// Aggregations computed once per render over the full set of currently
/// loaded history entries.
#[derive(Debug, Clone)]
pub(crate) struct Aggregates {
    /// One pair per [`RunState`] in `RunState::all()` order, even when
    /// the count is zero. Stable order keeps chart layouts steady.
    pub(crate) state_counts: Vec<(RunState, u64)>,
    /// Total run count across all states (mirror of
    /// `state_counts.iter().map(|(_, c)| c).sum()`).
    pub(crate) total: u64,
}

/// Aggregations for a single script.
#[derive(Debug, Clone)]
pub(crate) struct PerScriptAggregates {
    /// One pair per [`RunState`] in `RunState::all()` order, even when
    /// the count is zero.
    pub(crate) state_counts: Vec<(RunState, u64)>,
    pub(crate) total: u64,
    /// Last `PER_SCRIPT_DURATION_WINDOW` durations in chronological
    /// order (oldest first). Only runs whose state contributes a
    /// meaningful `duration_ms` are included.
    pub(crate) durations_ms: Vec<u64>,
    pub(crate) avg_ms: Option<u64>,
    pub(crate) p50_ms: Option<u64>,
    pub(crate) p95_ms: Option<u64>,
}

/// Linear-interpolation percentile over an unsorted slice. Returns
/// `None` for empty input. Uses an internal sorted copy so the caller
/// keeps its original ordering. `pct` is clamped into `[0.0, 1.0]`.
pub(crate) fn percentile(values: &[u64], pct: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted: Vec<u64> = values.to_vec();
    sorted.sort_unstable();
    if sorted.len() == 1 {
        return Some(sorted[0]);
    }
    let p = pct.clamp(0.0, 1.0);
    let pos = p * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        return Some(sorted[lo]);
    }
    let frac = pos - lo as f64;
    let lo_v = sorted[lo] as f64;
    let hi_v = sorted[hi] as f64;
    Some((lo_v + (hi_v - lo_v) * frac).round() as u64)
}

/// Aggregate state counts over `entries`.
pub(crate) fn aggregate(entries: &[RunRow]) -> Aggregates {
    let mut state_counts: Vec<(RunState, u64)> =
        RunState::all().iter().map(|s| (*s, 0u64)).collect();
    let mut total: u64 = 0;

    for entry in entries {
        if let Some(slot) = state_counts.iter_mut().find(|(s, _)| *s == entry.state) {
            slot.1 += 1;
            total += 1;
        }
    }

    Aggregates {
        state_counts,
        total,
    }
}

/// Build the "Top scripts by total run count" leaderboard.
///
/// Returns `(script_path, count)` pairs sorted by count descending,
/// with stable secondary ordering on the script path so two scripts
/// with the same count keep a deterministic relative order across
/// renders. The result is capped at `top_n`.
pub(crate) fn aggregate_top_scripts(entries: &[RunRow], top_n: usize) -> Vec<(String, u64)> {
    if entries.is_empty() || top_n == 0 {
        return Vec::new();
    }
    let mut counts: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for entry in entries {
        *counts.entry(entry.script_path.as_str()).or_insert(0) += 1;
    }
    let mut pairs: Vec<(String, u64)> = counts
        .into_iter()
        .map(|(path, count)| (path.to_string(), count))
        .collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    pairs.truncate(top_n);
    pairs
}

/// Aggregate state counts, recent durations, and percentiles for a
/// single script. `script_path` is matched exactly against
/// `RunRow::script_path` (history rows always carry absolute canonical
/// paths so a string comparison is sufficient).
pub(crate) fn aggregate_for_script(
    entries: &[RunRow],
    script_path: &str,
    last_n: usize,
) -> PerScriptAggregates {
    let mut state_counts: Vec<(RunState, u64)> =
        RunState::all().iter().map(|s| (*s, 0u64)).collect();
    let mut total: u64 = 0;

    // Collect (timestamp, duration) pairs for sorting; only durations
    // from states that actually carry a meaningful `duration_ms` count.
    let mut durations: Vec<(i64, u64)> = Vec::new();

    for entry in entries {
        if entry.script_path != script_path {
            continue;
        }
        if let Some(slot) = state_counts.iter_mut().find(|(s, _)| *s == entry.state) {
            slot.1 += 1;
            total += 1;
        }
        if matches!(
            entry.state,
            RunState::Completed | RunState::Failed | RunState::TimedOut
        ) {
            if let Some(d) = entry.duration_ms {
                if d >= 0 {
                    let ts = entry.finished_at.unwrap_or(entry.enqueued_at);
                    durations.push((ts, d as u64));
                }
            }
        }
    }

    // Sort chronologically (oldest first), then keep the last `last_n`.
    durations.sort_by_key(|(ts, _)| *ts);
    let last_n = last_n.max(1);
    let start = durations.len().saturating_sub(last_n);
    let durations_ms: Vec<u64> = durations[start..].iter().map(|(_, d)| *d).collect();

    let avg_ms = if durations_ms.is_empty() {
        None
    } else {
        let sum: u128 = durations_ms.iter().map(|d| *d as u128).sum();
        Some((sum / durations_ms.len() as u128) as u64)
    };
    let p50_ms = percentile(&durations_ms, 0.5);
    let p95_ms = percentile(&durations_ms, 0.95);

    PerScriptAggregates {
        state_counts,
        total,
        durations_ms,
        avg_ms,
        p50_ms,
        p95_ms,
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Entry point used by `ui::render_ui` when the History screen is in
/// `HistoryView::Dashboards`. The footer is rendered by the parent
/// `render_history` so the two views share a single hint line.
pub(crate) fn render_dashboards(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if app.history.entries.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Dashboards")
            .title_style(theme.text_secondary());
        let paragraph = Paragraph::new("No data to chart yet.")
            .block(block)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
        return;
    }

    let agg = aggregate(&app.history.entries);

    match app.history.dashboard_layout {
        DashboardLayout::Split => render_split_layout(frame, area, app, theme, &agg),
        DashboardLayout::ExpandedPerScript => render_expanded_layout(frame, area, app, theme),
    }
}

fn render_split_layout(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, agg: &Aggregates) {
    // Two stacked rows. Top half: side-by-side global cards
    // (Top runs + Donut by status). Bottom half: per-script panel.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    render_top_scripts_panel(frame, top[0], app, theme);
    render_global_status_panel(frame, top[1], theme, agg);
    render_per_script_panel(frame, chunks[1], app, theme, false);
}

fn render_expanded_layout(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    render_per_script_panel(frame, area, app, theme, true);
}

// ---------- Top scripts panel ----------

fn render_top_scripts_panel(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Top runs")
        .title_style(theme.text_secondary());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let top = aggregate_top_scripts(&app.history.entries, TOP_SCRIPTS_DEFAULT);
    if top.is_empty() {
        let placeholder = Paragraph::new("No runs yet.")
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(placeholder, inner);
        return;
    }

    let max = top.iter().map(|(_, c)| *c).max().unwrap_or(1).max(1);
    // Each entry uses three lines: name, bar, blank. Compute how many
    // entries we can fit and clip the list to that.
    let per_entry: u16 = 3;
    let max_entries = (inner.height / per_entry).max(1) as usize;
    let visible = &top[..top.len().min(max_entries)];

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible.len() * 3);
    let bar_color = Color::Cyan;
    let label_width = inner.width as usize;
    // Reserve a few cells at the right for the count text.
    let bar_max_width = inner.width.saturating_sub(6).max(1);

    for (path, count) in visible {
        let display = display_short_path(path, app);
        let truncated = truncate_for_width(&display, label_width);
        lines.push(Line::from(Span::styled(truncated, theme.text_secondary())));

        let cells = ((*count as f64 / max as f64) * bar_max_width as f64).ceil() as u16;
        let cells = cells.clamp(1, bar_max_width);
        let bar: String = "█".repeat(cells as usize);
        lines.push(Line::from(vec![
            Span::styled(
                bar,
                Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {}", count)),
        ]));
        lines.push(Line::from(""));
    }

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

fn display_short_path(script_path: &str, app: &App) -> String {
    app.display_path(&PathBuf::from(script_path))
}

fn truncate_for_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.chars().count() <= max_width {
        return text.to_string();
    }
    if max_width <= 1 {
        return text.chars().next().unwrap_or(' ').to_string();
    }
    let take = max_width - 1;
    let mut out: String = text.chars().take(take).collect();
    out.push('…');
    out
}

// ---------- Global donut by status ----------

fn render_global_status_panel(frame: &mut Frame, area: Rect, theme: &Theme, agg: &Aggregates) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Runs by status")
        .title_style(theme.text_secondary());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_state_bars(frame, inner, theme, &agg.state_counts, agg.total);
}

/// Render one horizontal bar per non-zero state, in the same visual
/// language as the "Top runs" panel: state name on its own line,
/// then a state-colored block bar with the count and percentage.
///
/// Bar width is normalized against the largest state count (so the
/// dominant state always fills the available width). The percentage
/// shown alongside is computed against `total` so the user reads
/// "X out of Y runs" rather than "X out of the dominant state".
fn render_state_bars(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    state_counts: &[(RunState, u64)],
    total: u64,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    if total == 0 {
        let placeholder = Paragraph::new("No runs yet.")
            .style(theme.text_secondary())
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(placeholder, area);
        return;
    }

    let max_count = state_counts
        .iter()
        .map(|(_, c)| *c)
        .max()
        .unwrap_or(1)
        .max(1);
    // Reserve cells on the right for the count and percentage text
    // (e.g. " 1234 (100%)" → 12 chars). Clamp to a sensible minimum.
    let reserve: u16 = 12;
    let bar_max_width = area.width.saturating_sub(reserve).max(1);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut first = true;
    for (state, count) in state_counts {
        if *count == 0 {
            continue;
        }
        if !first {
            lines.push(Line::from(""));
        }
        first = false;

        // Label line.
        lines.push(Line::from(Span::styled(
            state.as_str().to_string(),
            theme.text_secondary(),
        )));

        // Bar line.
        let cells = ((*count as f64 / max_count as f64) * bar_max_width as f64).ceil() as u16;
        let cells = cells.clamp(1, bar_max_width);
        let bar: String = "█".repeat(cells as usize);
        let pct = (*count as f64 / total as f64 * 100.0).round() as u32;
        lines.push(Line::from(vec![
            Span::styled(
                bar,
                Style::default()
                    .fg(state_color(*state))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {} ({}%)", count, pct)),
        ]));
    }

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

// ---------- Script-select per-script charts ----------

/// Render the per-script donut + duration panel for an arbitrary
/// script path. Used by the `ScriptSelect` screen so the right-hand
/// column shows execution stats next to the schema preview.
///
/// `script_path` is canonicalized internally so callers can pass the
/// raw `WorkspaceEntry::path` (which is absolute but may contain
/// symlinks) — `RunRow::script_path` is always canonicalized, so the
/// match is exact after this normalization.
///
/// `expanded` switches between a compact two-column layout (donut
/// left, duration right) and a fullscreen layout that gives the donut
/// more breathing room.
pub(crate) fn render_script_charts(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    script_path: &std::path::Path,
    expanded: bool,
) {
    let canonical = canonical_script_path_string(script_path);
    let matching: Vec<&_> = app
        .history
        .entries
        .iter()
        .filter(|r| r.script_path == canonical)
        .collect();

    let title = if expanded {
        format!("Activity: {}", app.display_path(script_path))
    } else {
        "Activity".to_string()
    };

    // Split vertically: activity grid on top (fixed height per period),
    // classic bar + duration dashboards on the bottom. When the area is
    // too short to fit the grid plus 5 rows for dashboards, keep only
    // the grid.
    let grid_h = super::activity_grid::widget_height(app.activity_period);
    if area.height < grid_h + 5 {
        super::activity_grid::render_activity_grid(
            frame,
            area,
            &matching,
            app.activity_period,
            theme,
            &title,
        );
        return;
    }

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(grid_h), Constraint::Min(5)])
        .split(area);

    super::activity_grid::render_activity_grid(
        frame,
        split[0],
        &matching,
        app.activity_period,
        theme,
        &title,
    );

    let agg = aggregate_for_script(&app.history.entries, &canonical, PER_SCRIPT_DURATION_WINDOW);
    let lower_block = Block::default()
        .borders(Borders::ALL)
        .title("State & duration")
        .title_style(theme.text_secondary());
    let lower_inner = lower_block.inner(split[1]);
    frame.render_widget(lower_block, split[1]);

    if agg.total == 0 {
        let placeholder = Paragraph::new(Line::from(vec![
            Span::raw("No runs yet. "),
            Span::styled("Press Enter to run it.", theme.text_secondary()),
        ]))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
        frame.render_widget(placeholder, lower_inner);
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(lower_inner);
    render_state_bars(frame, cols[0], theme, &agg.state_counts, agg.total);
    render_duration_panel(frame, cols[1], theme, &agg);
}

/// Canonicalize `script_path` to its absolute, symlink-resolved form
/// so it can be matched as a string against `RunRow::script_path`,
/// which is always canonicalized when written. Falls back to the raw
/// path on any I/O failure (e.g. the file was renamed/removed since
/// `list_entries` returned it).
fn canonical_script_path_string(script_path: &std::path::Path) -> String {
    std::fs::canonicalize(script_path)
        .unwrap_or_else(|_| script_path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

// ---------- Per-script panel (used inside the History Dashboards view) ----------

fn render_per_script_panel(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    expanded: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(per_script_title(app))
        .title_style(theme.text_secondary());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(entry) = app.history.entries.get(app.history.selection) else {
        let placeholder =
            Paragraph::new("Select a script in the List tab to see per-script charts.")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true });
        frame.render_widget(placeholder, inner);
        return;
    };

    let agg = aggregate_for_script(
        &app.history.entries,
        &entry.script_path,
        PER_SCRIPT_DURATION_WINDOW,
    );

    if agg.total == 0 {
        let placeholder = Paragraph::new("No runs for this script yet.")
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(placeholder, inner);
        return;
    }

    // Layout: state bars on the left, duration panel on the right.
    let cols = if expanded {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(inner)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner)
    };

    render_state_bars(frame, cols[0], theme, &agg.state_counts, agg.total);
    render_duration_panel(frame, cols[1], theme, &agg);
}

fn per_script_title(app: &App) -> String {
    let entry = match app.history.entries.get(app.history.selection) {
        Some(entry) => entry,
        None => return "Per script".to_string(),
    };
    let display = app.display_path(&PathBuf::from(&entry.script_path));
    format!("Per script: {}", display)
}

fn render_duration_panel(frame: &mut Frame, area: Rect, theme: &Theme, agg: &PerScriptAggregates) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Duration")
        .title_style(theme.text_secondary());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if agg.durations_ms.is_empty() {
        let placeholder = Paragraph::new("No completed runs yet.")
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(placeholder, inner);
        return;
    }

    let avg = agg.avg_ms.unwrap_or(0);
    let p50 = agg.p50_ms.unwrap_or(0);
    let p95 = agg.p95_ms.unwrap_or(0);
    let summary_lines = vec![
        Line::from(vec![
            Span::styled("avg ", theme.text_secondary()),
            Span::raw(format!("{} ms", avg)),
        ]),
        Line::from(vec![
            Span::styled("p50 ", theme.text_secondary()),
            Span::raw(format!("{} ms", p50)),
        ]),
        Line::from(vec![
            Span::styled("p95 ", theme.text_secondary()),
            Span::raw(format!("{} ms", p95)),
        ]),
    ];

    // Layout: 3-line summary header, then sparkline filling the rest.
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);
    let header = Paragraph::new(summary_lines);
    frame.render_widget(header, layout[0]);

    let max = agg.durations_ms.iter().copied().max().unwrap_or(1).max(1);
    let sparkline = Sparkline::default()
        .data(&agg.durations_ms)
        .max(max)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .bar_set(symbols::bar::NINE_LEVELS);
    frame.render_widget(sparkline, layout[1]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(script: &str, state: RunState, started_ms: i64, duration_ms: Option<i64>) -> RunRow {
        RunRow {
            run_id: format!("rid-{}-{}", script, started_ms),
            script_path: script.to_string(),
            script_name: None,
            args_json: "[]".into(),
            actor: "human".into(),
            reason: None,
            state,
            priority: 0,
            enqueued_at: started_ms,
            worker_id: None,
            lease_until: None,
            timeout_ms: None,
            cron_schedule_id: None,
            trigger: crate::runs::RunTrigger::Manual,
            started_at: Some(started_ms),
            finished_at: duration_ms.map(|d| started_ms + d),
            duration_ms,
            exit_code: None,
            success: None,
            stdout: String::new(),
            stderr: String::new(),
            error: None,
            parent_run_id: None,
            omakure_version: "test".into(),
        }
    }

    /// Anchor "today" at 2024-01-15 00:00:00 UTC.
    fn fixed_now_ms() -> i64 {
        1_705_276_800_000
    }

    #[test]
    fn aggregate_empty_returns_zero() {
        let agg = aggregate(&[]);
        assert_eq!(agg.total, 0);
        assert_eq!(agg.state_counts.len(), RunState::all().len());
        for (_, count) in &agg.state_counts {
            assert_eq!(*count, 0);
        }
    }

    #[test]
    fn aggregate_state_counts_match_groupby() {
        let entries = vec![
            row("a.sh", RunState::Completed, fixed_now_ms(), Some(100)),
            row("a.sh", RunState::Completed, fixed_now_ms(), Some(200)),
            row("b.sh", RunState::Failed, fixed_now_ms(), Some(50)),
            row("c.sh", RunState::Queued, fixed_now_ms(), None),
            row("c.sh", RunState::Running, fixed_now_ms(), None),
        ];
        let agg = aggregate(&entries);
        assert_eq!(agg.total, 5);
        let state = |s: RunState| {
            agg.state_counts
                .iter()
                .find(|(rs, _)| *rs == s)
                .map(|(_, c)| *c)
                .unwrap()
        };
        assert_eq!(state(RunState::Completed), 2);
        assert_eq!(state(RunState::Failed), 1);
        assert_eq!(state(RunState::Queued), 1);
        assert_eq!(state(RunState::Running), 1);
        assert_eq!(state(RunState::Cancelled), 0);
        assert_eq!(state(RunState::TimedOut), 0);
        assert_eq!(state(RunState::DeadLetter), 0);
    }

    #[test]
    fn aggregate_top_scripts_orders_by_count_then_name() {
        let entries = vec![
            row("/scripts/a.sh", RunState::Completed, 0, Some(10)),
            row("/scripts/a.sh", RunState::Completed, 0, Some(10)),
            row("/scripts/a.sh", RunState::Completed, 0, Some(10)),
            row("/scripts/b.sh", RunState::Failed, 0, Some(10)),
            row("/scripts/b.sh", RunState::Failed, 0, Some(10)),
            row("/scripts/c.sh", RunState::Queued, 0, None),
            row("/scripts/c.sh", RunState::Queued, 0, None),
        ];
        let top = aggregate_top_scripts(&entries, 10);
        // a.sh has 3 runs, b.sh and c.sh have 2 each.
        assert_eq!(top[0].0, "/scripts/a.sh");
        assert_eq!(top[0].1, 3);
        // Ties broken by script_path ascending, so b.sh comes before c.sh.
        assert_eq!(top[1].0, "/scripts/b.sh");
        assert_eq!(top[1].1, 2);
        assert_eq!(top[2].0, "/scripts/c.sh");
        assert_eq!(top[2].1, 2);
    }

    #[test]
    fn aggregate_top_scripts_caps_at_top_n() {
        let mut entries = Vec::new();
        for i in 0..10 {
            entries.push(row(
                &format!("/scripts/{:02}.sh", i),
                RunState::Completed,
                0,
                Some(10),
            ));
        }
        let top = aggregate_top_scripts(&entries, 3);
        assert_eq!(top.len(), 3);
    }

    #[test]
    fn aggregate_top_scripts_empty_input_returns_empty() {
        assert!(aggregate_top_scripts(&[], 10).is_empty());
    }

    #[test]
    fn aggregate_top_scripts_zero_top_n_returns_empty() {
        let entries = vec![row("/a.sh", RunState::Completed, 0, Some(10))];
        assert!(aggregate_top_scripts(&entries, 0).is_empty());
    }

    #[test]
    fn percentile_empty_is_none() {
        assert_eq!(percentile(&[], 0.5), None);
    }

    #[test]
    fn percentile_single_sample() {
        assert_eq!(percentile(&[42], 0.0), Some(42));
        assert_eq!(percentile(&[42], 0.5), Some(42));
        assert_eq!(percentile(&[42], 1.0), Some(42));
    }

    #[test]
    fn percentile_linear_interpolation() {
        let v = [10, 20, 30, 40, 50];
        assert_eq!(percentile(&v, 0.0), Some(10));
        assert_eq!(percentile(&v, 0.5), Some(30));
        assert_eq!(percentile(&v, 1.0), Some(50));
        assert_eq!(percentile(&v, 0.25), Some(20));
        assert_eq!(percentile(&v, 0.75), Some(40));
    }

    #[test]
    fn percentile_unsorted_input() {
        let v = [50, 10, 40, 20, 30];
        assert_eq!(percentile(&v, 0.5), Some(30));
    }

    #[test]
    fn per_script_avg_p50_p95() {
        let mut entries = Vec::new();
        for (i, d) in [10, 20, 30, 40, 50, 60, 70, 80, 90, 100].iter().enumerate() {
            entries.push(row(
                "a.sh",
                RunState::Completed,
                fixed_now_ms() + i as i64 * 1000,
                Some(*d),
            ));
        }
        let agg = aggregate_for_script(&entries, "a.sh", 20);
        assert_eq!(agg.total, 10);
        assert_eq!(agg.durations_ms.len(), 10);
        assert_eq!(agg.avg_ms, Some(55));
        assert_eq!(agg.p50_ms, Some(55));
        // f64 precision rounds 95.49999... down to 95.
        assert_eq!(agg.p95_ms, Some(95));
    }

    #[test]
    fn per_script_filters_other_scripts() {
        let entries = vec![
            row("a.sh", RunState::Completed, fixed_now_ms(), Some(100)),
            row("b.sh", RunState::Completed, fixed_now_ms(), Some(200)),
            row("a.sh", RunState::Failed, fixed_now_ms(), Some(150)),
        ];
        let agg = aggregate_for_script(&entries, "a.sh", 20);
        assert_eq!(agg.total, 2);
        assert_eq!(agg.durations_ms.len(), 2);
        assert_eq!(agg.avg_ms, Some(125));
    }

    #[test]
    fn per_script_excludes_states_without_duration() {
        let entries = vec![
            row("a.sh", RunState::Queued, fixed_now_ms(), None),
            row("a.sh", RunState::Running, fixed_now_ms(), None),
            row("a.sh", RunState::Cancelled, fixed_now_ms(), Some(50)),
            row("a.sh", RunState::Completed, fixed_now_ms(), Some(200)),
        ];
        let agg = aggregate_for_script(&entries, "a.sh", 20);
        assert_eq!(agg.total, 4);
        assert_eq!(agg.durations_ms, vec![200]);
        assert_eq!(agg.avg_ms, Some(200));
    }

    #[test]
    fn per_script_keeps_last_n_in_chronological_order() {
        let mut entries = Vec::new();
        for i in 0..30i64 {
            entries.push(row(
                "a.sh",
                RunState::Completed,
                fixed_now_ms() + i * 1000,
                Some((i + 1) * 10),
            ));
        }
        let agg = aggregate_for_script(&entries, "a.sh", 5);
        assert_eq!(agg.durations_ms, vec![260, 270, 280, 290, 300]);
        assert_eq!(agg.avg_ms, Some(280));
    }

    #[test]
    fn per_script_empty_durations_returns_none() {
        let entries = vec![
            row("a.sh", RunState::Queued, fixed_now_ms(), None),
            row("a.sh", RunState::Running, fixed_now_ms(), None),
        ];
        let agg = aggregate_for_script(&entries, "a.sh", 20);
        assert_eq!(agg.total, 2);
        assert!(agg.durations_ms.is_empty());
        assert_eq!(agg.avg_ms, None);
        assert_eq!(agg.p50_ms, None);
        assert_eq!(agg.p95_ms, None);
    }

    #[test]
    fn truncate_for_width_handles_short_long_zero() {
        assert_eq!(truncate_for_width("hello", 10), "hello");
        assert_eq!(truncate_for_width("hello world", 6), "hello…");
        assert_eq!(truncate_for_width("hello", 0), "");
        assert_eq!(truncate_for_width("hello", 1), "h");
    }

    // --- Rendering tests via TestBackend ---

    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::use_cases::ScriptService;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tempfile::TempDir;

    fn make_svc(tmp: &TempDir) -> ScriptService {
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        ScriptService::new(Box::new(repo), Box::new(runner))
    }

    fn make_history(tmp: &TempDir) -> Vec<RunRow> {
        let root = tmp.path().display().to_string();
        vec![
            row(
                &format!("{}/deploy.sh", root),
                RunState::Completed,
                1000,
                Some(150),
            ),
            row(
                &format!("{}/deploy.sh", root),
                RunState::Failed,
                2000,
                Some(200),
            ),
            row(
                &format!("{}/deploy.sh", root),
                RunState::Completed,
                3000,
                Some(100),
            ),
            row(
                &format!("{}/setup.sh", root),
                RunState::Completed,
                1500,
                Some(300),
            ),
            row(&format!("{}/setup.sh", root), RunState::Running, 4000, None),
        ]
    }

    #[test]
    fn snapshot_render_dashboards_split() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let history = make_history(&tmp);
        let mut app = crate::adapters::tui::app::App::test_new(&svc, ws, vec![], history);
        app.history.view = crate::adapters::tui::app::HistoryView::Dashboards;
        let theme = app.theme.clone();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_dashboards(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_render_dashboards_empty() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = crate::adapters::tui::app::App::test_new(&svc, ws, vec![], vec![]);
        app.history.view = crate::adapters::tui::app::HistoryView::Dashboards;
        let theme = app.theme.clone();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_dashboards(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_render_script_charts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let history = make_history(&tmp);
        let script = format!("{}/deploy.sh", tmp.path().display());
        let app = crate::adapters::tui::app::App::test_new(&svc, ws, vec![], history);
        let theme = app.theme.clone();

        let backend = TestBackend::new(60, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render_script_charts(
                    f,
                    f.size(),
                    &app,
                    &theme,
                    std::path::Path::new(&script),
                    false,
                )
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
