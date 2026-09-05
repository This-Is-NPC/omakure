# CLI / HTTP parity

<!-- BEGIN GENERATED PARITY -->

Manifest schema version: **1**.

| Class | Entries |
|---|---:|
| exact | 35 |
| semantic-mismatch | 9 |
| cli-only | 20 |
| http-only | 8 |

| Entry | Class | Operation family | CLI IDs | HTTP IDs | Behavior case |
|---|---|---|---|---|---|
| <a id="battery-add"></a>`battery-add` | semantic-mismatch | `battery` | battery add | POST /v1/batteries | mismatch.battery-add |

> `battery-https-policy`: CLI — CLI accepts local/SSH sources; HTTP permits HTTPS sources only; HTTP — HTTP rejects non-HTTPS sources..

| <a id="battery-inspect"></a>`battery-inspect` | exact | `battery` | battery inspect | GET /v1/batteries/:battery_id | exact.battery-inspect |
| <a id="battery-install"></a>`battery-install` | semantic-mismatch | `battery` | battery install | POST /v1/batteries/:battery_id/scripts/:script_id/install | mismatch.battery-install |

> `battery-https-policy`: CLI — CLI install is local and Unix-capable; HTTP is authenticated operator initiation; HTTP — HTTP adds route authentication and lifecycle limits..

| <a id="battery-list"></a>`battery-list` | exact | `battery` | battery list | GET /v1/batteries | exact.battery-list |
| <a id="battery-remove"></a>`battery-remove` | exact | `battery` | battery remove | DELETE /v1/batteries/:battery_id | exact.battery-remove |
| <a id="battery-scripts"></a>`battery-scripts` | exact | `battery` | battery scripts | GET /v1/batteries/:battery_id/scripts | exact.battery-scripts |
| <a id="battery-sync"></a>`battery-sync` | semantic-mismatch | `battery` | battery sync | POST /v1/batteries/:battery_id/sync | mismatch.battery-sync |

> `battery-https-policy`: CLI — CLI can use configured source transports; HTTP sync is HTTPS-only; HTTP — HTTP enforces HTTPS transport policy..

| <a id="cli-api"></a>`cli-api` | cli-only | `api` | api |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-completion"></a>`cli-completion` | cli-only | `completion` | completion |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-help-ai"></a>`cli-help-ai` | cli-only | `help-ai` | help-ai |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-history-tail"></a>`cli-history-tail` | cli-only | `history.tail` | history tail |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-init"></a>`cli-init` | cli-only | `init` | init |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-node-authority-create"></a>`cli-node-authority-create` | cli-only | `node.authority.create` | node authority create |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-node-authority-issue"></a>`cli-node-authority-issue` | cli-only | `node.authority.issue` | node authority issue |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-node-authority-show"></a>`cli-node-authority-show` | cli-only | `node.authority.show` | node authority show |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-node-baseline-create-key"></a>`cli-node-baseline-create-key` | cli-only | `node.baseline.create-key` | node baseline create-key |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-node-baseline-publish"></a>`cli-node-baseline-publish` | cli-only | `node.baseline.publish` | node baseline publish |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-node-direct-probe"></a>`cli-node-direct-probe` | cli-only | `node.direct-probe` | node direct-probe |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-node-reset"></a>`cli-node-reset` | cli-only | `node.reset` | node reset |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-node-serve"></a>`cli-node-serve` | cli-only | `node.serve` | node serve |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-queue-worker"></a>`cli-queue-worker` | cli-only | `queue.worker` | queue worker |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-run"></a>`cli-run` | cli-only | `run` | run |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-serve"></a>`cli-serve` | cli-only | `serve` | serve |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-token-generate"></a>`cli-token-generate` | cli-only | `token.generate` | token generate |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-trace"></a>`cli-trace` | cli-only | `trace` | trace |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-uninstall"></a>`cli-uninstall` | cli-only | `uninstall` | uninstall |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="cli-update"></a>`cli-update` | cli-only | `update` | update |  | — |
> Rationale: CLI-only local lifecycle or trust operation.

| <a id="config"></a>`config` | semantic-mismatch | `config` | config | GET /v1/config | mismatch.config |

> `config-redaction`: CLI — CLI returns active environment values; HTTP redacts values; HTTP — HTTP returns redacted values..

| <a id="describe"></a>`describe` | exact | `describe` | describe | GET /v1/scripts/:script_id | exact.describe |
| <a id="doctor"></a>`doctor` | exact | `doctor` | doctor | GET /v1/doctor | exact.doctor |
| <a id="env-activate"></a>`env-activate` | exact | `env` | env activate | POST /v1/envs/:name/activate | exact.env-activate |
| <a id="env-create"></a>`env-create` | exact | `env` | env create | POST /v1/envs | exact.env-create |
| <a id="env-deactivate"></a>`env-deactivate` | exact | `env` | env deactivate | DELETE /v1/envs/active | exact.env-deactivate |
| <a id="env-delete"></a>`env-delete` | exact | `env` | env delete | DELETE /v1/envs/:name | exact.env-delete |
| <a id="env-list"></a>`env-list` | exact | `env` | env list | GET /v1/envs | exact.env-list |
| <a id="env-remove"></a>`env-remove` | exact | `env` | env remove | DELETE /v1/envs/:name/params/:key | exact.env-remove |
| <a id="env-replace"></a>`env-replace` | exact | `env` | env replace | PUT /v1/envs/:name | exact.env-replace |
| <a id="env-set"></a>`env-set` | exact | `env` | env set | PATCH /v1/envs/:name<br>PUT /v1/envs/:name/params/:key | exact.env-set |
| <a id="env-show"></a>`env-show` | exact | `env` | env show | GET /v1/envs/:name | exact.env-show |
| <a id="history-list"></a>`history-list` | exact | `history` | history list | GET /v1/runs | exact.history-list |
| <a id="history-show"></a>`history-show` | exact | `history` | history show | GET /v1/runs/:run_id | exact.history-show |
| <a id="history-stats"></a>`history-stats` | exact | `history` | history stats<br>queue stats | GET /v1/queue/stats | exact.history-stats |
| <a id="history-traces"></a>`history-traces` | exact | `history` | history traces | GET /v1/runs/:run_id/traces | exact.history-traces |
| <a id="http-admin-status"></a>`http-admin-status` | http-only | `admin-status` |  | GET /v1/admin/status | — |
> Rationale: HTTP-only admin process status.

| <a id="http-health"></a>`http-health` | http-only | `health` |  | GET /v1/health | — |
> Rationale: HTTP-only liveness.

| <a id="http-node-enrollments"></a>`http-node-enrollments` | http-only | `node enrollments` |  | GET /v1/node/enrollments | — |
> Rationale: HTTP-only pending enrollment listing.

| <a id="http-ready"></a>`http-ready` | http-only | `ready` |  | GET /v1/ready | — |
> Rationale: HTTP-only readiness.

| <a id="http-secrets"></a>`http-secrets` | http-only | `secrets` |  | GET /v1/secrets | — |
> Rationale: HTTP-only secret metadata only.

| <a id="http-tree-path"></a>`http-tree-path` | http-only | `tree-path` |  | GET /v1/tree/:path | — |
> Rationale: HTTP-only safe script tree browsing.

| <a id="http-tree-root"></a>`http-tree-root` | http-only | `tree-root` |  | GET /v1/tree | — |
> Rationale: HTTP-only safe script tree browsing.

| <a id="http-workspace"></a>`http-workspace` | http-only | `workspace` |  | GET /v1/workspace | — |
> Rationale: HTTP-only service workspace metadata.

| <a id="node-baseline-push"></a>`node-baseline-push` | exact | `node baseline` | node baseline push | POST /v1/node/baselines | exact.node-baseline-push |
| <a id="node-baseline-rollback"></a>`node-baseline-rollback` | exact | `node baseline` | node baseline rollback | POST /v1/node/baseline/rollback | exact.node-baseline-rollback |
| <a id="node-capabilities"></a>`node-capabilities` | exact | `node` | node capabilities | PATCH /v1/node/peers/:node_id/capabilities | exact.node-capabilities |
| <a id="node-cue"></a>`node-cue` | semantic-mismatch | `node` | node cue | POST /v1/node/cues | mismatch.node-cue |

> `cue-session`: CLI — CLI may dial directly or use the running service session; HTTP — HTTP dispatches only over the held service session..

| <a id="node-discovery"></a>`node-discovery` | semantic-mismatch | `node` | node discovery | GET /v1/node/discovery | mismatch.node-discovery |

> `discovery-snapshot`: CLI — CLI performs a fresh bounded LAN scan; HTTP — HTTP returns the service live in-memory snapshot..

| <a id="node-enroll-apply"></a>`node-enroll-apply` | semantic-mismatch | `node enroll apply` | node enroll apply | POST /v1/node/enrollment/bundle | mismatch.node-enroll-apply |

> `enroll-token-source`: CLI — CLI reads a local bootstrap token file; HTTP — HTTP receives the one-time token through the authenticated request flow..

| <a id="node-enroll-approve"></a>`node-enroll-approve` | exact | `node enroll` | node enroll approve | POST /v1/node/enrollments/:node_id/approve | exact.node-enroll-approve |
| <a id="node-enroll-reject"></a>`node-enroll-reject` | exact | `node enroll` | node enroll reject | POST /v1/node/enrollments/:node_id/reject | exact.node-enroll-reject |
| <a id="node-enroll-request"></a>`node-enroll-request` | semantic-mismatch | `node enroll request` | node enroll request | POST /v1/node/enrollments | mismatch.node-enroll-request |

> `enroll-stage-dial`: CLI — CLI creates and dials a request directly; HTTP — HTTP stages the request for the service transport..

| <a id="node-health"></a>`node-health` | exact | `node` | node health | GET /v1/node/health | exact.node-health |
| <a id="node-init"></a>`node-init` | exact | `node` | node init | POST /v1/node/init | exact.node-init |
| <a id="node-peers"></a>`node-peers` | exact | `node` | node peers | GET /v1/node/peers | exact.node-peers |
| <a id="node-revoke"></a>`node-revoke` | exact | `node` | node revoke | POST /v1/node/peers/:node_id/revoke | exact.node-revoke |
| <a id="node-signals"></a>`node-signals` | exact | `node` | node signals | GET /v1/node/signals | exact.node-signals |
| <a id="node-status"></a>`node-status` | exact | `node` | node status | GET /v1/node/status | exact.node-status |
| <a id="node-trust"></a>`node-trust` | exact | `node` | node trust | POST /v1/node/peers | exact.node-trust |
| <a id="queue-add"></a>`queue-add` | exact | `queue` | queue add | POST /v1/runs | exact.queue-add |
| <a id="queue-cancel"></a>`queue-cancel` | exact | `queue` | queue cancel | POST /v1/runs/:run_id/cancel | exact.queue-cancel |
| <a id="queue-dead-letter"></a>`queue-dead-letter` | exact | `queue` | queue dead-letter | POST /v1/runs/:run_id/dead-letter | exact.queue-dead-letter |
| <a id="scripts"></a>`scripts` | exact | `scripts` | scripts | GET /v1/scripts | exact.scripts |
| <a id="search"></a>`search` | semantic-mismatch | `search` | search | GET /v1/search | mismatch.search |

> `search-refresh-limits`: CLI — CLI refreshes its local index; HTTP uses the existing index and bounds query/tag input; HTTP — HTTP does not refresh and applies stricter limits..


<!-- END GENERATED PARITY -->
