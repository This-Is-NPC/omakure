use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::App;
use super::super::theme::Theme;
use super::spinner::{spinner_span, SpinnerKind};

pub(crate) fn render_running(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let script_name = app
        .field_input
        .selected_script
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>");
    let args = format_running_args(app);

    let lines = vec![
        Line::from(vec![
            spinner_span(SpinnerKind::Sand, app.tick, theme),
            Span::raw("Running script..."),
        ]),
        Line::from(""),
        Line::from(format!("Script: {}", script_name)),
        Line::from(format!("Args: {}", args)),
        Line::from(""),
        Line::from("Please wait."),
    ];
    let block = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Executing"))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    frame.render_widget(block, area);
}

fn format_running_args(app: &App) -> String {
    if app.field_input.args.is_empty() {
        return "-".to_string();
    }
    let mut args = app.field_input.args.clone();
    for field in app
        .field_input
        .fields
        .iter()
        .filter(|field| field.is_secret())
    {
        let flag = field
            .arg
            .clone()
            .unwrap_or_else(|| format!("--{}", field.name));
        for idx in 0..args.len() {
            if args[idx] == flag {
                if let Some(value) = args.get_mut(idx + 1) {
                    *value = crate::adapters::environments::MASKED_ENV_VALUE.to_string();
                }
            } else if args[idx].starts_with(&format!("{}=", flag)) {
                args[idx] = format!(
                    "{}={}",
                    flag,
                    crate::adapters::environments::MASKED_ENV_VALUE
                );
            }
        }
    }
    args.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::use_cases::ScriptService;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn snapshot_running_with_script() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.field_input.selected_script = Some(PathBuf::from("/scripts/deploy.sh"));
        app.field_input.args = vec!["--target".to_string(), "prod".to_string()];
        app.tick = 3;

        let theme = app.theme.clone();
        let backend = TestBackend::new(50, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_running(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn running_args_mask_secret_fields() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.field_input.fields = vec![crate::domain::Field {
            name: "TOKEN".into(),
            prompt: None,
            kind: "secret".into(),
            order: None,
            required: None,
            arg: Some("--token".into()),
            default: None,
            choices: None,
        }];
        app.field_input.args = vec!["--token".into(), "plain_secret".into()];

        let rendered = format_running_args(&app);

        assert!(rendered.contains("****"));
        assert!(!rendered.contains("plain_secret"));
    }
}
