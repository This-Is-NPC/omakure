# How it works

1. Omakure resolves one workspace root from `--scripts-dir`, environment
   overrides, debug defaults, and platform defaults.
2. The shared filesystem repository scans supported script extensions while
   excluding metadata and `.omakureignore` matches.
3. Each script may embed a PascalCase JSON schema. The schema describes fields,
   outputs, queue cases, and an optional cron `Schedule`.
4. CLI commands and HTTP routes call the same operations. CLI output is human
   readable or the stable JSON envelope; HTTP adds authentication, policy, and
   status mapping.
5. Direct `run` inserts a running row. `queue add` inserts a queued row and a
   worker claims it atomically. `serve` and the engine scheduler enqueue due
   scheduled rows. All paths use the same executor.
6. The executor resolves the runtime, injects managed/per-run environments,
   sets reserved run variables, captures and redacts output, refreshes a lease,
   and records the terminal state in `.history/runs.sqlite`.
7. `history` and `queue stats` expose state; `trace` and `history traces` expose
   structured progress without requiring direct database access.

## Deployable engine

`omakure engine` starts the HTTP server and, unless disabled, an in-process
worker pool and schedule scanner. `/v1/health` reports liveness and `/v1/ready`
reports minimal readiness. Optional readiness flags require the configured
worker or scheduler loop to be alive. Shutdown stops HTTP acceptance first,
then scheduling/claiming, then drains workers.

## Script lifecycle

```text
init -> scripts/describe -> run or queue add -> worker/executor -> history/traces
                                      \-> serve/engine scheduler for Schedule
```

Use `help-ai` once per binary version to discover the complete command tree and
data shapes. Use `config` and `doctor` before execution when an agent needs to
verify workspace paths or host runtimes.
