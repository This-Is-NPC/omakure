# Documentation index

Start here if you want a map of the docs. For quick start and requirements, see the repo `README.md`.

## For users

- `installation.md`: install, update, uninstall, and version pinning.
- `usage.md`: CLI commands and common flows.
- `scheduling.md`: cron scheduler (`omakure serve`) — lifecycle, overlap protection, systemd autostart, observability.
- `workspace.md`: workspace layout, runtime state (`runs.sqlite`), daemon artifacts.
- `scripts-path.md`: default scripts path and the full resolution precedence.
- `environments.md`: environment documents (`.omakure/envs/`) and session `omakure.conf`.
- `lua-widgets.md`: how to render `index.lua` widgets in the TUI.
- `how-it-works.md`: overview of the manual / queued / scheduled execution paths.
- `how-to-create-a-script.md`: step-by-step script guide incl. the `Schedule` block.

## For AI integrators

- `ai-interface.md`: JSON envelope contract, AI-facing verbs, error codes, data shapes.

## For contributors

- `development.md`: dev workflow, `mise` tasks, repo layout.
- `architecture.md`: stack, patterns, code metrics, infrastructure.
- `requirements.md`: implemented functional/non-functional/business rules (file-referenced).
- `release-artifacts.md`: release archive naming requirements.
- `tui-screens-and-widgets.md`: TUI screen flow, per-screen keybindings, and widget inventory.
- `env-injection-spec.md`: env precedence table, var-expansion grammar, and the secret-non-persistence invariant (gates injection/parser work).
