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
| `omakure node authority` | CLI-only. Holding the key that mints fleet membership is an operator act at the machine that holds it, and an HTTP route would put issuing behind a bearer token — a token that could then enroll anyone into the fleet. The blast radius of the two surfaces is not comparable, so they are not given parity. |
| `omakure node cue` | **Has parity**: `POST /v1/node/cues`, under `node:write`. Originally planned as CLI-only on the grounds that a route was a fifth authorization surface for no safety gain. That premise turned out to be wrong on both halves: no new capability was needed, and the route is the *only* path that works in a managed fleet, because a separate process cannot dial a peer the running service already has a session with. Every authorization gate stays on the receiving node; the route decides only whether this operator may ask. |
| `omakure node baseline` | **Partial parity, deliberately.** `push` has parity: `POST /v1/node/baselines`, under `node:write`, and it is the *only* path — a baseline goes to a Performer this node already conducts, so there is no first-contact case a direct dial would serve, and adding one would be a second way into the responder for a case that does not exist. `rollback` has parity too: `POST /v1/node/baseline/rollback`, also under `node:write`, and it is local on both surfaces — it puts *this* machine back on the one baseline it retained and reaches no peer. `create-key` and `publish` are CLI-only and stay that way: both touch the baseline publisher key, which is held apart from everything the service process can reach, and a route that could sign would put authoring code and ordering runs back in one place. |
| `omakure node direct-probe` | CLI-only, deliberately, and unlike `node cue`. A Cue is an instruction that has to arrive, so relaying it over the session the service holds is the only path that works. A probe is a question, and for a peer the service is already connected to the answer is already in hand: that session was built by the same handshake, identity check, and authorization a probe performs, and it is dropped when any of them stops holding. `GET /v1/node/status` already reports it, under the narrower `node:read`. A relayed probe would also write a `probe_accepted` audit row for a handshake that never happened. Instead the CLI names the collision: a probe refused because the service holds the session fails with `already_exists` and points at `node status`, rather than the bare `transport_internal` the closed stream used to produce. |
| `omakure queue worker` | Long-running daemon process, not request/response API behavior. |
| `omakure update` | Replaces binary and copies release scripts. |
| `omakure uninstall` | Destructive local operation. |

## CLI-only surfaces

| CLI | Reason |
|---|---|
| `omakure api` | Starts the HTTP server itself. |
| `omakure completion <shell>` | Shell integration emitted to local stdout. |
