use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use super::super::app::App;
use super::super::theme::Theme;
use super::activity_grid::render_activity_grid;

/// Width of the list column on the Schedules screen. Mirrors the
/// history screen's "list on the left / details on the right" layout.
const LIST_MIN_WIDTH: u16 = 40;

pub(crate) fn render_schedules(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let outer = Block::default().borders(Borders::ALL).title("Schedules");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(inner);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(list_width(chunks[0].width)),
            Constraint::Min(20),
        ])
        .split(chunks[0]);

    render_list(frame, body[0], app, theme);
    render_right(frame, body[1], app, theme);

    let flash = app.schedules.flash.as_deref().unwrap_or("");
    let footer = Paragraph::new(format!(
        "Up/Down move, Space toggle, Tab cycle period, r refresh, Esc back    {flash}"
    ))
    .style(theme.text_secondary());
    frame.render_widget(footer, chunks[1]);
}

fn list_width(total: u16) -> u16 {
    // Keep list around 45% of the area, clamped so the grid always has
    // room. On very narrow terminals fall back to the minimum.
    let pct = total.saturating_mul(45) / 100;
    pct.max(LIST_MIN_WIDTH).min(total.saturating_sub(25))
}

fn render_list(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if app.schedules.entries.is_empty() {
        let msg = Paragraph::new(
            "No scheduled scripts. Add a \"Schedule\" block to a script's embedded schema to see it here.",
        )
        .style(theme.text_secondary())
        .block(Block::default().borders(Borders::ALL).title("Scripts"));
        frame.render_widget(msg, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Script"),
        Cell::from("Cron"),
        Cell::from("On"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = app
        .schedules
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let enabled_span = if entry.enabled {
                Span::styled("●", Style::default().add_modifier(Modifier::BOLD))
            } else {
                Span::styled("○", theme.text_secondary())
            };
            let row = Row::new(vec![
                Cell::from(entry.display_name.clone()),
                Cell::from(entry.cron.clone()),
                Cell::from(Line::from(enabled_span)),
            ]);
            if i == app.schedules.selection {
                row.style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                row
            }
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(14),
            Constraint::Length(3),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title("Scripts"));
    frame.render_widget(table, area);
}

fn render_right(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let Some(entry) = app.schedules.entries.get(app.schedules.selection).cloned() else {
        let msg = Paragraph::new("Select a scheduled script on the left to see its activity.")
            .style(theme.text_secondary())
            .block(Block::default().borders(Borders::ALL).title("Activity"));
        frame.render_widget(msg, area);
        return;
    };

    let grid_h = super::activity_grid::widget_height(app.activity_period);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Length(grid_h),
            Constraint::Min(0),
        ])
        .split(area);

    // Summary header: next run, last run status, totals.
    let next = entry
        .next_run
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "—".to_string());
    // Match every run of the selected script (canonical path). The
    // Schedules view's purpose is to show "what happened for this
    // scheduled script" — using trigger=Scheduled would exclude manual
    // runs of the same script, which the user may still want to see.
    let canonical =
        std::fs::canonicalize(&entry.script_path).unwrap_or_else(|_| entry.script_path.clone());
    let canonical_str = canonical.to_string_lossy().to_string();
    let matching: Vec<&_> = app
        .history
        .entries
        .iter()
        .filter(|r| r.script_path == canonical_str)
        .collect();
    let scheduled_count = matching
        .iter()
        .filter(|r| matches!(r.trigger, crate::runs::RunTrigger::Scheduled))
        .count();

    let info_lines = vec![
        Line::from(vec![
            Span::styled("Script: ", theme.text_secondary()),
            Span::raw(entry.display_name.clone()),
        ]),
        Line::from(vec![
            Span::styled("Cron: ", theme.text_secondary()),
            Span::raw(entry.cron.clone()),
            Span::raw("    "),
            Span::styled("Enabled: ", theme.text_secondary()),
            Span::raw(if entry.enabled { "yes" } else { "no" }),
        ]),
        Line::from(vec![
            Span::styled("Next run: ", theme.text_secondary()),
            Span::raw(next),
        ]),
        Line::from(vec![
            Span::styled("Total runs: ", theme.text_secondary()),
            Span::raw(matching.len().to_string()),
            Span::styled(" (scheduled: ", theme.text_secondary()),
            Span::raw(scheduled_count.to_string()),
            Span::styled(")", theme.text_secondary()),
        ]),
    ];
    let info =
        Paragraph::new(info_lines).block(Block::default().borders(Borders::ALL).title("Overview"));
    frame.render_widget(info, chunks[0]);

    render_activity_grid(
        frame,
        chunks[1],
        &matching,
        app.activity_period,
        theme,
        "Activity",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::tui::app::{ScheduleEntry, SchedulesState};
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::use_cases::ScriptService;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn render_empty_schedules_no_panic() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        let theme = app.theme.clone();
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_schedules(f, f.size(), &mut app, &theme))
            .unwrap();
    }

    #[test]
    fn render_populated_schedules_no_panic() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.schedules = SchedulesState {
            entries: vec![
                ScheduleEntry {
                    script_path: PathBuf::from("/tmp/a.sh"),
                    display_name: "a.sh".into(),
                    cron: "*/5 * * * *".into(),
                    enabled: true,
                    next_run: Some(chrono::Utc::now()),
                },
                ScheduleEntry {
                    script_path: PathBuf::from("/tmp/b.sh"),
                    display_name: "b.sh".into(),
                    cron: "@hourly".into(),
                    enabled: false,
                    next_run: None,
                },
            ],
            selection: 0,
            list_state: ratatui::widgets::ListState::default(),
            flash: Some("toggled".into()),
        };
        let theme = app.theme.clone();
        let backend = TestBackend::new(160, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_schedules(f, f.size(), &mut app, &theme))
            .unwrap();
    }
}
