# CLI / HTTP parity

This document records the CLI surfaces that currently have equivalent behavior
through shared operations and the HTTP management API.

## Legend

| Status | Meaning |
|---|---|
| Full | HTTP exposes the same core behavior through shared operations. |
| Partial | HTTP exposes a safe subset or equivalent workflow. |
| CLI-only | The command is inherently local CLI behavior. |

## Implemented parity

| CLI | Operation | HTTP | Status | Notes |
|---|---|---|---|---|
| `omakure config --json` | `config_summary` | `GET /v1/config` | Full | HTTP redacts active env values. |
| workspace summary | `workspace_summary` | `GET /v1/workspace` | Full | HTTP client workspace metadata. |
| `omakure doctor` | `doctor_report` | `GET /v1/doctor` | Full | CLI human output differs. |
| `omakure scripts --json` | `list_scripts` | `GET /v1/scripts` | Full | Shared tag filtering. |
| `omakure describe <script> --json` | `describe_script` | `GET /v1/scripts/{script_id}` | Full | Shared path resolution and schema errors. |
| schema projection | `describe_script` | `GET /v1/scripts/{script_id}/schema` | Full | HTTP convenience projection. |
| script tree browsing | `list_tree` | `GET /v1/tree`, `GET /v1/tree/{path}` | Full | HTTP-only safe browsing surface. |
| script content read | `read_script_content` | `GET /v1/scripts/{script_id}/content` | Full | UTF-8 only, 1 MiB cap, metadata paths blocked. |
| `omakure search <query> --json` | `search_scripts` | `GET /v1/search?q=...` | Partial | HTTP uses existing index and caps query/tag input. |
| `omakure history list --json` | `list_runs` | `GET /v1/runs` | Full | Shared filters. |
| `omakure history show <run_id> --json` | `show_run` | `GET /v1/runs/{run_id}` | Full | Stable missing-run errors. |
| `omakure history traces <run_id> --json` | `list_traces` | `GET /v1/runs/{run_id}/traces` | Full | Shared trace filters. |
| `omakure history stats --json` | `run_stats` | `GET /v1/queue/stats` | Partial | Same data, queue-named HTTP route. |
| `omakure queue stats --json` | `queue_stats` | `GET /v1/queue/stats` | Full | Same aggregate as history stats. |
| `omakure queue add <script> --json` | `enqueue_run` | `POST /v1/runs` | Full | HTTP exposes enqueue, not inline run. |
| `omakure queue cancel <run_id> --json` | `cancel_run` | `POST /v1/runs/{run_id}/cancel` | Full | Shared state transition. |
| `omakure queue dead-letter <run_id> --json` | `dead_letter_run` | `POST /v1/runs/{run_id}/dead-letter` | Full | Shared state transition. |
| `omakure env list --json` | `list_envs` | `GET /v1/envs` | Full | Lists managed `.omakure/envs/*.conf` files. |
| `omakure env create <name> ... --json` | `create_env` | `POST /v1/envs` | Full | Shared env name/key validation. |
| `omakure env show <name> --json` | `show_env` | `GET /v1/envs/{name}` | Full | Values are redacted by sensitive key. |
| `omakure env replace <name> ... --json` | `replace_env` | `PUT /v1/envs/{name}` | Full | Rewrites the managed file atomically. |
| `omakure env set <name> KEY=VALUE --json` | `set_param` | `PATCH /v1/envs/{name}`, `PUT /v1/envs/{name}/params/{key}` | Full | Updates or adds a key. |
| `omakure env remove <name> <key> --json` | `remove_param` | `DELETE /v1/envs/{name}/params/{key}` | Full | Removes one key. |
| `omakure env activate <name> --json` | `activate_env` | `POST /v1/envs/{name}/activate` | Full | Writes `.omakure/envs/active`. |
| `omakure env deactivate --json` | `deactivate_env` | `DELETE /v1/envs/active` | Full | Clears active env. |
| `omakure env delete <name> --json` | `delete_env` | `DELETE /v1/envs/{name}` | Full | Deletes the managed file and clears active if needed. |
| `omakure battery list --json` | `list_batteries` | `GET /v1/batteries` | Full | Shared Battery operation. |
| `omakure battery add <url> --json` | `add_battery` | `POST /v1/batteries` | Partial | HTTP accepts HTTPS sources only. |
| `omakure battery sync <name> --json` | `sync_battery` | `POST /v1/batteries/{battery_id}/sync` | Full | Shared operation. |
| `omakure battery inspect <name> --json` | `inspect_battery` | `GET /v1/batteries/{battery_id}` | Full | Shared operation. |
| `omakure battery scripts <name> --json` | `list_battery_scripts` | `GET /v1/batteries/{battery_id}/scripts` | Full | Shared operation. |
| `omakure battery install <name> <script-id> --json` | `install_battery_script` | `POST /v1/batteries/{battery_id}/scripts/{script_id}/install` | Full | Shared operation. |
| `omakure battery remove <name> --json` | `remove_battery` | `DELETE /v1/batteries/{battery_id}` | Full | Supports `remove_cache` query option. |

## Not exposed over HTTP

| CLI | Reason |
|---|---|
| `omakure help-ai` | Capability discovery is a local CLI JSON surface. |
| `omakure serve --status` | systemd user-unit inspection is local host process state. |
| `omakure history stats --json` | Same aggregate is exposed through `GET /v1/queue/stats`; there is no separate `/v1/runs/stats` route. |
| `omakure run <script>` | Inline synchronous execution is CLI-only. HTTP enqueues with `POST /v1/runs`. |
| `omakure init <script>` | Script creation is local CLI-only. |
| `omakure trace` | Trace writes are script-authored CLI calls using `OMAKURE_RUN_ID`. |
| `omakure serve` lifecycle | Host process control is CLI-only. |
| `omakure node cue` | Dispatching a Cue is CLI-only. An HTTP route would be a fifth authorization surface to keep in step, for no safety gain, on a feature whose point is bounding what a remote caller can reach. |
| `omakure queue worker` | Long-running daemon process, not request/response API behavior. |
| `omakure update` | Replaces binary and copies release scripts. |
| `omakure uninstall` | Destructive local operation. |

## CLI-only surfaces

| CLI | Reason |
|---|---|
| `omakure api` | Starts the HTTP server itself. |
| `omakure completion <shell>` | Shell integration emitted to local stdout. |
