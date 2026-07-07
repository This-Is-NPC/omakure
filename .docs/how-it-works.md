# How it works (overview)

1) Scripts live anywhere under `~/Documents/omakure-scripts` (Windows: `%USERPROFILE%\Documents\omakure-scripts`) with `.bash`, `.sh`, `.ps1`, or `.py` extensions.
2) Scripts embed their schema as a commented JSON block between `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END`. An optional `Schedule` block turns the script into a self-contained automation unit.
3) If a folder has `index.lua`, the TUI renders the widget in the header panel. See `lua-widgets.md`.
4) The TUI reads schemas, shows Outputs/Queue/Schedule details when present, prompts for values, and runs the script with args.
5) Execution paths:
   - **Manual** — pick a script in the TUI or run `omakure run <script>` (`trigger = Manual`).
   - **Queued** — `omakure queue add <script>` produces a `queued` row; `omakure queue worker` drains the queue.
   - **Scheduled** — `omakure serve` daemon reads each script's `Schedule`, fires on the cron expression, and enqueues rows with `trigger = Scheduled`. See `usage.md`.
6) Every run is recorded in `<workspace>/.history/runs.sqlite` (state machine + structured traces). Query with `omakure history list|show|traces` or the TUI `H` screen.

## Script index (examples)

| Script | Description |
| --- | --- |
| `scripts/azure/rg-list-all.bash` | List resource groups with CreatedAt, LastModified, and CreatedBy. |
| `scripts/azure/rg-details.bash` | Show resource group details and list resources with CreatedAt, LastModified, and CreatedBy. |
| `scripts/azure/rg-delete.bash` | Delete a resource group and all resources inside it. |
