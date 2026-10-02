use super::enqueue::{EnqueueOptions, enqueue_in_transaction};
use super::ids::generate_run_id;
use super::{RunState, RunTrigger, RunsError};
use crate::util::time::unix_millis;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepSnapshot {
    pub name: String,
    pub script_path: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSnapshot {
    pub battery_id: String,
    pub battery_version: String,
    pub battery_commit: String,
    pub workflow_name: String,
    pub steps: Vec<WorkflowStepSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl WorkflowState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepRun {
    pub step_index: usize,
    pub name: String,
    pub script_path: String,
    pub run_id: Option<String>,
    pub state: Option<RunState>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub workflow_id: String,
    pub battery_id: String,
    pub battery_version: String,
    pub battery_commit: String,
    pub workflow_name: String,
    pub actor: String,
    pub state: WorkflowState,
    pub current_step: usize,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub steps: Vec<WorkflowStepRun>,
}

fn db_error(operation: &'static str, source: rusqlite::Error) -> RunsError {
    RunsError::Sqlite { operation, source }
}

/// Persist the approved snapshot and make only its first run claimable.
pub fn start_workflow(
    conn: &Connection,
    snapshot: WorkflowSnapshot,
    actor: &str,
) -> Result<WorkflowRun, RunsError> {
    if snapshot.steps.len() < 2 {
        return Err(RunsError::InvalidEnqueue(
            "Workflow requires at least two steps",
        ));
    }
    if snapshot.battery_id.is_empty()
        || snapshot.battery_version.is_empty()
        || snapshot.battery_commit.is_empty()
        || snapshot.workflow_name.is_empty()
    {
        return Err(RunsError::InvalidEnqueue("Workflow provenance is required"));
    }
    if snapshot.steps.iter().any(|step| {
        step.name.is_empty() || step.script_path.is_empty() || step.content_hash.is_empty()
    }) {
        return Err(RunsError::InvalidEnqueue(
            "Workflow step requires a name, path and hash",
        ));
    }
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|source| db_error("Begin workflow failed", source))?;
    let workflow_id = generate_run_id();
    tx.execute(
        "INSERT INTO workflow_runs (workflow_id, battery_id, battery_version, battery_commit, workflow_name, actor, state, current_step, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', 0, ?7)",
        params![workflow_id, snapshot.battery_id, snapshot.battery_version, snapshot.battery_commit, snapshot.workflow_name, actor, unix_millis()],
    ).map_err(|source| db_error("Insert workflow failed", source))?;
    for (index, step) in snapshot.steps.iter().enumerate() {
        tx.execute(
            "INSERT INTO workflow_steps (workflow_id, step_index, name, script_path, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                workflow_id,
                index as i64,
                step.name,
                step.script_path,
                step.content_hash
            ],
        )
        .map_err(|source| db_error("Insert workflow step failed", source))?;
    }
    enqueue_step(&tx, &workflow_id, 0, actor, None)?;
    tx.commit()
        .map_err(|source| db_error("Commit workflow failed", source))?;
    get_workflow(conn, &workflow_id)?.ok_or_else(|| RunsError::NotFound(workflow_id))
}

fn enqueue_step(
    conn: &Connection,
    workflow_id: &str,
    index: usize,
    actor: &str,
    parent_run_id: Option<String>,
) -> Result<(), RunsError> {
    let (name, path, hash): (String, String, String) = conn
        .query_row(
            "SELECT name, script_path, content_hash FROM workflow_steps
         WHERE workflow_id = ?1 AND step_index = ?2 AND run_id IS NULL",
            params![workflow_id, index as i64],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|source| db_error("Read workflow step failed", source))?;
    let run = enqueue_in_transaction(
        conn,
        &path,
        &[],
        EnqueueOptions {
            actor: actor.to_owned(),
            reason: Some(format!("workflow:{workflow_id}:{index}")),
            parent_run_id,
            script_name: Some(name),
            omakure_version: env!("CARGO_PKG_VERSION").to_owned(),
            trigger: RunTrigger::Workflow,
            script_content_hash: Some(hash),
            allowed_secret_refs: Some(Vec::new()),
            ..Default::default()
        },
    )?;
    conn.execute(
        "UPDATE workflow_steps SET run_id = ?1 WHERE workflow_id = ?2 AND step_index = ?3 AND run_id IS NULL",
        params![run.run_id, workflow_id, index as i64],
    ).map_err(|source| db_error("Link workflow run failed", source))?;
    Ok(())
}

pub fn get_workflow(
    conn: &Connection,
    workflow_id: &str,
) -> Result<Option<WorkflowRun>, RunsError> {
    let workflow = conn.query_row(
        "SELECT workflow_id, battery_id, battery_version, battery_commit, workflow_name, actor, state, current_step, created_at, finished_at
         FROM workflow_runs WHERE workflow_id = ?1",
        [workflow_id],
        |row| {
            let state: String = row.get(6)?;
            Ok(WorkflowRun {
                workflow_id: row.get(0)?, battery_id: row.get(1)?, battery_version: row.get(2)?,
                battery_commit: row.get(3)?, workflow_name: row.get(4)?, actor: row.get(5)?,
                state: WorkflowState::parse(&state)?, current_step: row.get::<_, i64>(7)? as usize,
                created_at: row.get(8)?, finished_at: row.get(9)?, steps: Vec::new(),
            })
        },
    ).optional().map_err(|source| db_error("Read workflow failed", source))?;
    let Some(mut workflow) = workflow else {
        return Ok(None);
    };
    let mut statement = conn
        .prepare(
            "SELECT s.step_index, s.name, s.script_path, s.run_id, r.state, r.error, r.reason
         FROM workflow_steps s LEFT JOIN runs r ON r.run_id = s.run_id
         WHERE s.workflow_id = ?1 ORDER BY s.step_index",
        )
        .map_err(|source| db_error("Prepare workflow steps failed", source))?;
    let rows = statement
        .query_map([workflow_id], |row| {
            let state: Option<String> = row.get(4)?;
            let error: Option<String> = row.get(5)?;
            let reason: Option<String> = row.get(6)?;
            let step_index: i64 = row.get(0)?;
            let cancellation_error = (state.as_deref() == Some("cancelled") && error.is_none())
                .then(|| {
                    let default_reason = format!("workflow:{workflow_id}:{step_index}");
                    reason
                        .filter(|value| value != &default_reason)
                        .unwrap_or_else(|| "cancelled by user".to_owned())
                });
            Ok(WorkflowStepRun {
                step_index: step_index as usize,
                name: row.get(1)?,
                script_path: row.get(2)?,
                run_id: row.get(3)?,
                state: state
                    .map(|value| {
                        value
                            .parse::<RunState>()
                            .map_err(|_| rusqlite::Error::InvalidQuery)
                    })
                    .transpose()?,
                error: error.or(cancellation_error),
            })
        })
        .map_err(|source| db_error("Query workflow steps failed", source))?;
    for row in rows {
        workflow
            .steps
            .push(row.map_err(|source| db_error("Read workflow step status failed", source))?);
    }
    Ok(Some(workflow))
}

/// Advance once after a terminal step. The immediate transaction makes repeated
/// worker callbacks and restart recovery idempotent.
pub fn advance_workflow_for_run(
    conn: &Connection,
    run_id: &str,
) -> Result<Option<WorkflowRun>, RunsError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|source| db_error("Begin workflow advancement failed", source))?;
    let workflow_id: Option<String> = tx
        .query_row(
            "SELECT workflow_id FROM workflow_steps WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| db_error("Find workflow for run failed", source))?;
    let Some(workflow_id) = workflow_id else {
        return Ok(None);
    };
    advance_locked(&tx, &workflow_id, run_id)?;
    tx.commit()
        .map_err(|source| db_error("Commit workflow advancement failed", source))?;
    get_workflow(conn, &workflow_id)
}

fn advance_locked(conn: &Connection, workflow_id: &str, run_id: &str) -> Result<(), RunsError> {
    let current: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT w.current_step, w.actor, w.state FROM workflow_runs w WHERE w.workflow_id = ?1",
            [workflow_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|source| db_error("Read workflow cursor failed", source))?;
    let Some((current_step, actor, state)) = current else {
        return Ok(());
    };
    if state != WorkflowState::Running.as_str() {
        return Ok(());
    }
    let current_run_id: String = conn
        .query_row(
            "SELECT run_id FROM workflow_steps WHERE workflow_id = ?1 AND step_index = ?2",
            params![workflow_id, current_step],
            |row| row.get(0),
        )
        .map_err(|source| db_error("Read current workflow run failed", source))?;
    if current_run_id != run_id {
        return Ok(());
    }
    let run_state: String = conn
        .query_row(
            "SELECT state FROM runs WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(|source| db_error("Read workflow run state failed", source))?;
    let run_state: RunState = run_state
        .parse()
        .map_err(|_| RunsError::InvalidEnqueue("Stored run state is invalid"))?;
    match run_state {
        RunState::Queued | RunState::Running => return Ok(()),
        RunState::Completed => {
            let next = current_step + 1;
            let has_next: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM workflow_steps WHERE workflow_id = ?1 AND step_index = ?2)",
                params![workflow_id, next], |row| row.get(0),
            ).map_err(|source| db_error("Find next workflow step failed", source))?;
            if has_next {
                enqueue_step(
                    conn,
                    workflow_id,
                    next as usize,
                    &actor,
                    Some(run_id.to_owned()),
                )?;
                conn.execute(
                    "UPDATE workflow_runs SET current_step = ?1 WHERE workflow_id = ?2",
                    params![next, workflow_id],
                )
                .map_err(|source| db_error("Advance workflow cursor failed", source))?;
                return Ok(());
            }
            finish(conn, workflow_id, WorkflowState::Completed)?;
        }
        RunState::Cancelled => finish(conn, workflow_id, WorkflowState::Cancelled)?,
        RunState::Failed | RunState::TimedOut | RunState::DeadLetter => {
            finish(conn, workflow_id, WorkflowState::Failed)?
        }
    }
    Ok(())
}

fn finish(conn: &Connection, workflow_id: &str, state: WorkflowState) -> Result<(), RunsError> {
    conn.execute(
        "UPDATE workflow_runs SET state = ?1, finished_at = ?2 WHERE workflow_id = ?3 AND state = 'running'",
        params![state.as_str(), unix_millis(), workflow_id],
    ).map_err(|source| db_error("Finish workflow failed", source))?;
    Ok(())
}

/// Reconcile terminal steps that finished just before a process stopped.
pub fn recover_workflows(conn: &Connection) -> Result<Vec<WorkflowRun>, RunsError> {
    let mut statement = conn.prepare(
        "SELECT s.run_id FROM workflow_runs w JOIN workflow_steps s
         ON s.workflow_id = w.workflow_id AND s.step_index = w.current_step
         JOIN runs r ON r.run_id = s.run_id
         WHERE w.state = 'running' AND r.state IN ('completed', 'failed', 'cancelled', 'timed_out', 'dead_letter')",
    ).map_err(|source| db_error("Prepare workflow recovery failed", source))?;
    let run_ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|source| db_error("Query workflow recovery failed", source))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|source| db_error("Read workflow recovery failed", source))?;
    drop(statement);
    run_ids
        .into_iter()
        .map(|run_id| {
            advance_workflow_for_run(conn, &run_id)?.ok_or_else(|| RunsError::RunNotFound(run_id))
        })
        .collect()
}
