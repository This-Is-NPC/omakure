use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::path::PathBuf;

use super::super::app::{App, ExecutionStatus};
use super::super::theme::Theme;
use super::common::status_label_and_style;
use super::history::format_run_output;

pub(crate) fn render_run_result(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let lines = render_lines(app, theme);
    let view_height = chunks[0].height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(view_height);
    if max_scroll == 0 {
        app.run_output_scroll = 0;
    } else if app.run_output_scroll as usize > max_scroll {
        app.run_output_scroll = max_scroll.min(u16::MAX as usize) as u16;
    }

    let output = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Last run output"),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.run_output_scroll, 0));
    frame.render_widget(output, chunks[0]);

    let footer = Paragraph::new("Up/Down to scroll, PgUp/PgDn, Enter/Esc to return, h for history")
        .style(theme.text_secondary());
    frame.render_widget(footer, chunks[1]);
}

fn render_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let entry = match app.history.entries.first() {
        Some(entry) => entry,
        None => {
            lines.push(Line::from("No script output yet."));
            return lines;
        }
    };

    let name = app.display_path(&PathBuf::from(&entry.script_path));
    let args = match serde_json::from_str::<Vec<String>>(&entry.args_json) {
        Ok(args) if args.is_empty() => "-".to_string(),
        Ok(args) => args.join(" "),
        Err(_) => entry.args_json.clone(),
    };
    let status = ExecutionStatus::from_run(entry);
    let (status_label, status_style) = status_label_and_style(&status, theme);
    lines.push(Line::from(format!("Script: {}", name)));
    lines.push(Line::from(format!("Args: {}", args)));
    lines.push(Line::from(vec![
        Span::raw("Status: "),
        Span::styled(status_label, status_style),
    ]));
    lines.push(Line::from(""));
    let output = format_run_output(entry);
    if output.trim().is_empty() {
        lines.push(Line::from("(no output)"));
    } else {
        lines.extend(output.lines().map(|line| Line::from(line.to_string())));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::runs::{RunRow, RunState};
    use crate::use_cases::ScriptService;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tempfile::TempDir;

    fn make_run_row(success: bool, stdout: &str, stderr: &str) -> RunRow {
        RunRow {
            run_id: "test-run".into(),
            script_path: "/scripts/deploy.sh".into(),
            script_name: None,
            args_json: r#"["--target","prod"]"#.into(),
            actor: "human".into(),
            reason: None,
            state: if success {
                RunState::Completed
            } else {
                RunState::Failed
            },
            priority: 0,
            enqueued_at: 0,
            worker_id: None,
            lease_until: None,
            timeout_ms: None,
            cron_schedule_id: None,
            started_at: Some(0),
            finished_at: Some(100),
            duration_ms: Some(100),
            exit_code: if success { Some(0) } else { Some(1) },
            success: Some(success),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            error: None,
            parent_run_id: None,
            omakure_version: "test".into(),
        }
    }

    #[test]
    fn snapshot_run_result_success() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let row = make_run_row(true, "Deployed successfully\n", "");
        let mut app = App::test_new(&svc, ws, vec![], vec![row]);

        let theme = app.theme.clone();
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_run_result(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_run_result_failure() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let row = make_run_row(false, "", "Error: connection refused\n");
        let mut app = App::test_new(&svc, ws, vec![], vec![row]);

        let theme = app.theme.clone();
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_run_result(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_run_result_no_history() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);

        let theme = app.theme.clone();
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_run_result(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
