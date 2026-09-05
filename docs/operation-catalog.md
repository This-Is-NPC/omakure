<!-- GENERATED FILE: scripts/tasks/operation-catalog --write -->
# Operation catalog

Catalog version: `1.0.0`; schema version: `1`.

| Operation ID | Parity entry | Plane | Remote eligibility | Effect | Mutability | CLI | HTTP |
|---|---|---|---|---|---|---|---|
| `op.battery-add` | `battery-add` | `domain` | `control-execute` | `execute` | `non-idempotent` | `battery add` | `POST /v1/batteries` |
| `op.battery-inspect` | `battery-inspect` | `domain` | `control-observe` | `read` | `immutable` | `battery inspect` | `GET /v1/batteries/:battery_id` |
| `op.battery-install` | `battery-install` | `domain` | `control-execute` | `execute` | `non-idempotent` | `battery install` | `POST /v1/batteries/:battery_id/scripts/:script_id/install` |
| `op.battery-list` | `battery-list` | `domain` | `control-observe` | `read` | `immutable` | `battery list` | `GET /v1/batteries` |
| `op.battery-remove` | `battery-remove` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `battery remove` | `DELETE /v1/batteries/:battery_id` |
| `op.battery-scripts` | `battery-scripts` | `domain` | `control-observe` | `read` | `immutable` | `battery scripts` | `GET /v1/batteries/:battery_id/scripts` |
| `op.battery-sync` | `battery-sync` | `domain` | `control-execute` | `execute` | `non-idempotent` | `battery sync` | `POST /v1/batteries/:battery_id/sync` |
| `op.cli-api` | `cli-api` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `api` | — |
| `op.cli-completion` | `cli-completion` | `domain` | `local-only` | `read` | `immutable` | `completion` | — |
| `op.cli-help-ai` | `cli-help-ai` | `domain` | `local-only` | `read` | `immutable` | `help-ai` | — |
| `op.cli-history-tail` | `cli-history-tail` | `domain` | `local-only` | `read` | `immutable` | `history tail` | — |
| `op.cli-init` | `cli-init` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `init` | — |
| `op.cli-node-authority-create` | `cli-node-authority-create` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `node authority create` | — |
| `op.cli-node-authority-issue` | `cli-node-authority-issue` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `node authority issue` | — |
| `op.cli-node-authority-show` | `cli-node-authority-show` | `domain` | `local-only` | `read` | `immutable` | `node authority show` | — |
| `op.cli-node-baseline-create-key` | `cli-node-baseline-create-key` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `node baseline create-key` | — |
| `op.cli-node-baseline-publish` | `cli-node-baseline-publish` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `node baseline publish` | — |
| `op.cli-node-direct-probe` | `cli-node-direct-probe` | `domain` | `local-only` | `read` | `immutable` | `node direct-probe` | — |
| `op.cli-node-reset` | `cli-node-reset` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `node reset` | — |
| `op.cli-node-serve` | `cli-node-serve` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `node serve` | — |
| `op.cli-queue-worker` | `cli-queue-worker` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `queue worker` | — |
| `op.cli-run` | `cli-run` | `local-lifecycle` | `local-only` | `execute` | `non-idempotent` | `run` | — |
| `op.cli-serve` | `cli-serve` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `serve` | — |
| `op.cli-token-generate` | `cli-token-generate` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `token generate` | — |
| `op.cli-trace` | `cli-trace` | `local-lifecycle` | `local-only` | `execute` | `non-idempotent` | `trace` | — |
| `op.cli-uninstall` | `cli-uninstall` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `uninstall` | — |
| `op.cli-update` | `cli-update` | `local-lifecycle` | `local-only` | `lifecycle` | `non-idempotent` | `update` | — |
| `op.config` | `config` | `domain` | `control-observe` | `read` | `immutable` | `config` | `GET /v1/config` |
| `op.describe` | `describe` | `domain` | `control-observe` | `read` | `immutable` | `describe` | `GET /v1/scripts/:script_id` |
| `op.doctor` | `doctor` | `domain` | `control-observe` | `read` | `immutable` | `doctor` | `GET /v1/doctor` |
| `op.env-activate` | `env-activate` | `domain` | `control-converge` | `mutate` | `idempotent` | `env activate` | `POST /v1/envs/:name/activate` |
| `op.env-create` | `env-create` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `env create` | `POST /v1/envs` |
| `op.env-deactivate` | `env-deactivate` | `domain` | `control-converge` | `mutate` | `idempotent` | `env deactivate` | `DELETE /v1/envs/active` |
| `op.env-delete` | `env-delete` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `env delete` | `DELETE /v1/envs/:name` |
| `op.env-list` | `env-list` | `domain` | `control-observe` | `read` | `immutable` | `env list` | `GET /v1/envs` |
| `op.env-remove` | `env-remove` | `domain` | `control-converge` | `mutate` | `idempotent` | `env remove` | `DELETE /v1/envs/:name/params/:key` |
| `op.env-replace` | `env-replace` | `domain` | `control-converge` | `mutate` | `idempotent` | `env replace` | `PUT /v1/envs/:name` |
| `op.env-set` | `env-set` | `domain` | `control-converge` | `mutate` | `idempotent` | `env set` | `PATCH /v1/envs/:name`, `PUT /v1/envs/:name/params/:key` |
| `op.env-show` | `env-show` | `domain` | `control-observe` | `read` | `immutable` | `env show` | `GET /v1/envs/:name` |
| `op.history-list` | `history-list` | `domain` | `control-observe` | `read` | `immutable` | `history list` | `GET /v1/runs` |
| `op.history-show` | `history-show` | `domain` | `control-observe` | `read` | `immutable` | `history show` | `GET /v1/runs/:run_id` |
| `op.history-stats` | `history-stats` | `domain` | `control-observe` | `read` | `immutable` | `history stats`, `queue stats` | `GET /v1/queue/stats` |
| `op.history-traces` | `history-traces` | `domain` | `control-observe` | `read` | `immutable` | `history traces` | `GET /v1/runs/:run_id/traces` |
| `op.http-admin-status` | `http-admin-status` | `service-observation` | `control-observe` | `observe` | `immutable` | — | `GET /v1/admin/status` |
| `op.http-health` | `http-health` | `service-observation` | `control-observe` | `observe` | `immutable` | — | `GET /v1/health` |
| `op.http-node-enrollments` | `http-node-enrollments` | `service-observation` | `control-observe` | `observe` | `immutable` | — | `GET /v1/node/enrollments` |
| `op.http-ready` | `http-ready` | `service-observation` | `control-observe` | `observe` | `immutable` | — | `GET /v1/ready` |
| `op.http-secrets` | `http-secrets` | `service-observation` | `control-observe` | `observe` | `immutable` | — | `GET /v1/secrets` |
| `op.http-tree-path` | `http-tree-path` | `service-observation` | `control-observe` | `observe` | `immutable` | — | `GET /v1/tree/:path` |
| `op.http-tree-root` | `http-tree-root` | `service-observation` | `control-observe` | `observe` | `immutable` | — | `GET /v1/tree` |
| `op.http-workspace` | `http-workspace` | `service-observation` | `control-observe` | `observe` | `immutable` | — | `GET /v1/workspace` |
| `op.node-baseline-push` | `node-baseline-push` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `node baseline push` | `POST /v1/node/baselines` |
| `op.node-baseline-rollback` | `node-baseline-rollback` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `node baseline rollback` | `POST /v1/node/baseline/rollback` |
| `op.node-capabilities` | `node-capabilities` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `node capabilities` | `PATCH /v1/node/peers/:node_id/capabilities` |
| `op.node-cue` | `node-cue` | `domain` | `control-execute` | `execute` | `non-idempotent` | `node cue` | `POST /v1/node/cues` |
| `op.node-discovery` | `node-discovery` | `domain` | `control-observe` | `observe` | `immutable` | `node discovery` | `GET /v1/node/discovery` |
| `op.node-enroll-apply` | `node-enroll-apply` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `node enroll apply` | `POST /v1/node/enrollment/bundle` |
| `op.node-enroll-approve` | `node-enroll-approve` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `node enroll approve` | `POST /v1/node/enrollments/:node_id/approve` |
| `op.node-enroll-reject` | `node-enroll-reject` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `node enroll reject` | `POST /v1/node/enrollments/:node_id/reject` |
| `op.node-enroll-request` | `node-enroll-request` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `node enroll request` | `POST /v1/node/enrollments` |
| `op.node-health` | `node-health` | `domain` | `control-observe` | `read` | `immutable` | `node health` | `GET /v1/node/health` |
| `op.node-init` | `node-init` | `domain` | `control-execute` | `lifecycle` | `non-idempotent` | `node init` | `POST /v1/node/init` |
| `op.node-peers` | `node-peers` | `domain` | `control-observe` | `read` | `immutable` | `node peers` | `GET /v1/node/peers` |
| `op.node-revoke` | `node-revoke` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `node revoke` | `POST /v1/node/peers/:node_id/revoke` |
| `op.node-signals` | `node-signals` | `domain` | `control-observe` | `read` | `immutable` | `node signals` | `GET /v1/node/signals` |
| `op.node-status` | `node-status` | `domain` | `control-observe` | `read` | `immutable` | `node status` | `GET /v1/node/status` |
| `op.node-trust` | `node-trust` | `domain` | `control-execute` | `mutate` | `non-idempotent` | `node trust` | `POST /v1/node/peers` |
| `op.queue-add` | `queue-add` | `domain` | `control-execute` | `execute` | `non-idempotent` | `queue add` | `POST /v1/runs` |
| `op.queue-cancel` | `queue-cancel` | `domain` | `control-execute` | `execute` | `non-idempotent` | `queue cancel` | `POST /v1/runs/:run_id/cancel` |
| `op.queue-dead-letter` | `queue-dead-letter` | `domain` | `control-execute` | `execute` | `non-idempotent` | `queue dead-letter` | `POST /v1/runs/:run_id/dead-letter` |
| `op.scripts` | `scripts` | `domain` | `control-observe` | `read` | `immutable` | `scripts` | `GET /v1/scripts` |
| `op.search` | `search` | `domain` | `control-observe` | `read` | `immutable` | `search` | `GET /v1/search` |

Total operations: 72.
