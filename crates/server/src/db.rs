use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use spotwatt_core::{Policy, Priority};
use sqlx::sqlite::{SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};

use crate::model::{Job, JobStatus, NewJob, Repeat};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS jobs (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT    NOT NULL,
    command          TEXT    NOT NULL,
    policy_kind      TEXT    NOT NULL,
    threshold_nok    REAL,
    duration_minutes INTEGER NOT NULL DEFAULT 60,
    deadline         INTEGER,
    power_watts      REAL,
    priority         TEXT    NOT NULL DEFAULT 'normal',
    repeat_kind      TEXT    NOT NULL DEFAULT 'none',
    status           TEXT    NOT NULL DEFAULT 'pending',
    scheduled_start  INTEGER,
    created_at       INTEGER NOT NULL,
    started_at       INTEGER,
    finished_at      INTEGER,
    exit_code        INTEGER,
    output           TEXT,
    est_cost_nok     REAL
);
"#;

pub async fn init(url: &str) -> Result<SqlitePool> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(url)
        .await?;
    sqlx::query(SCHEMA).execute(&pool).await?;
    // Add columns introduced after the original schema. Ignore the "duplicate
    // column" error on databases that already have them.
    let _ = sqlx::query("ALTER TABLE jobs ADD COLUMN repeat_kind TEXT NOT NULL DEFAULT 'none'")
        .execute(&pool)
        .await;
    Ok(pool)
}

/// On startup, any job still marked `running` is an orphan from a previous
/// process that died mid-run: nothing is executing it, yet it occupies a
/// concurrency slot forever. Mark such jobs failed so slots free up and the
/// state is honest. Returns how many were reconciled.
pub async fn reconcile_orphans(pool: &SqlitePool, now: i64) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE jobs
         SET status = 'failed', finished_at = ?,
             output = COALESCE(output, '') || '\n[interrupted: server restarted while running]'
         WHERE status = 'running'",
    )
    .bind(now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Total nameplate power (watts) of all currently running jobs. Jobs with no
/// declared power contribute nothing. Used to enforce the site power budget.
pub async fn running_power_watts(pool: &SqlitePool) -> Result<f64> {
    let row = sqlx::query(
        "SELECT COALESCE(SUM(power_watts), 0.0) AS w FROM jobs WHERE status = 'running'",
    )
    .fetch_one(pool)
    .await?;
    Ok(row.get::<f64, _>("w"))
}

fn from_ts(secs: Option<i64>) -> Option<DateTime<Utc>> {
    secs.and_then(|s| Utc.timestamp_opt(s, 0).single())
}

fn policy_to_db(p: &Policy) -> (&'static str, Option<f64>) {
    match p {
        Policy::Immediate => ("immediate", None),
        Policy::Threshold { max_nok_per_kwh } => ("threshold", Some(*max_nok_per_kwh)),
        Policy::CheapestWindow => ("cheapest", None),
    }
}

fn priority_to_db(p: Priority) -> &'static str {
    match p {
        Priority::Low => "low",
        Priority::Normal => "normal",
        Priority::High => "high",
        Priority::Critical => "critical",
    }
}

fn row_to_job(row: &SqliteRow) -> Job {
    let kind: String = row.get("policy_kind");
    let threshold: Option<f64> = row.get("threshold_nok");
    let policy = match kind.as_str() {
        "immediate" => Policy::Immediate,
        "threshold" => Policy::Threshold {
            max_nok_per_kwh: threshold.unwrap_or(0.0),
        },
        _ => Policy::CheapestWindow,
    };

    let prio: String = row.get("priority");
    let priority = match prio.as_str() {
        "low" => Priority::Low,
        "high" => Priority::High,
        "critical" => Priority::Critical,
        _ => Priority::Normal,
    };

    let status: String = row.get("status");
    let repeat: String = row.get("repeat_kind");

    Job {
        id: row.get("id"),
        name: row.get("name"),
        command: row.get("command"),
        policy,
        duration_minutes: row.get("duration_minutes"),
        deadline: from_ts(row.get("deadline")),
        power_watts: row.get("power_watts"),
        priority,
        repeat: Repeat::from_db(&repeat),
        status: JobStatus::from_db(&status),
        scheduled_start: from_ts(row.get("scheduled_start")),
        created_at: from_ts(Some(row.get::<i64, _>("created_at"))).unwrap_or_else(Utc::now),
        started_at: from_ts(row.get("started_at")),
        finished_at: from_ts(row.get("finished_at")),
        exit_code: row.get("exit_code"),
        output: row.get("output"),
        est_cost_nok: row.get("est_cost_nok"),
    }
}

pub async fn create_job(pool: &SqlitePool, n: NewJob) -> Result<Job> {
    let (kind, threshold) = policy_to_db(&n.policy);
    let res = sqlx::query(
        "INSERT INTO jobs
            (name, command, policy_kind, threshold_nok, duration_minutes, deadline,
             power_watts, priority, repeat_kind, status, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
    )
    .bind(&n.name)
    .bind(&n.command)
    .bind(kind)
    .bind(threshold)
    .bind(n.duration_minutes)
    .bind(n.deadline.map(|d| d.timestamp()))
    .bind(n.power_watts)
    .bind(priority_to_db(n.priority))
    .bind(n.repeat.as_str())
    .bind(n.created_at.timestamp())
    .execute(pool)
    .await?;

    let id = res.last_insert_rowid();
    get_job(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("inserted job {id} vanished"))
}

/// Schedule the next occurrence of a recurring job, given the one that just
/// finished. Daily jobs reappear as a fresh pending row with any deadline
/// rolled forward 24h. Non-recurring jobs do nothing. Returns the new job, if
/// one was created.
pub async fn create_next_occurrence(pool: &SqlitePool, finished: &Job) -> Result<Option<Job>> {
    let step = match finished.repeat {
        Repeat::None => return Ok(None),
        Repeat::Daily => chrono::Duration::days(1),
    };
    let next = NewJob {
        name: finished.name.clone(),
        command: finished.command.clone(),
        policy: finished.policy,
        duration_minutes: finished.duration_minutes,
        deadline: finished.deadline.map(|d| d + step),
        power_watts: finished.power_watts,
        priority: finished.priority,
        repeat: finished.repeat,
        created_at: Utc::now(),
    };
    create_job(pool, next).await.map(Some)
}

pub async fn list_jobs(pool: &SqlitePool) -> Result<Vec<Job>> {
    let rows = sqlx::query("SELECT * FROM jobs ORDER BY created_at DESC, id DESC")
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_job).collect())
}

pub async fn pending_jobs(pool: &SqlitePool) -> Result<Vec<Job>> {
    let rows = sqlx::query("SELECT * FROM jobs WHERE status = 'pending' ORDER BY id ASC")
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_job).collect())
}

pub async fn get_job(pool: &SqlitePool, id: i64) -> Result<Option<Job>> {
    let row = sqlx::query("SELECT * FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_job))
}

pub async fn running_count(pool: &SqlitePool) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM jobs WHERE status = 'running'")
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("n"))
}

pub async fn set_scheduled_start(pool: &SqlitePool, id: i64, ts: Option<i64>) -> Result<()> {
    sqlx::query("UPDATE jobs SET scheduled_start = ? WHERE id = ?")
        .bind(ts)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Atomically claim a pending job for running. Returns true if this call won
/// the claim (status was still 'pending'), guarding against double launches.
pub async fn claim_for_running(pool: &SqlitePool, id: i64, started: i64) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE jobs SET status = 'running', started_at = ? WHERE id = ? AND status = 'pending'",
    )
    .bind(started)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn mark_finished(
    pool: &SqlitePool,
    id: i64,
    status: JobStatus,
    finished: i64,
    exit_code: Option<i64>,
    output: &str,
    est_cost: Option<f64>,
) -> Result<()> {
    sqlx::query(
        "UPDATE jobs
         SET status = ?, finished_at = ?, exit_code = ?, output = ?, est_cost_nok = ?
         WHERE id = ?",
    )
    .bind(status.as_str())
    .bind(finished)
    .bind(exit_code)
    .bind(output)
    .bind(est_cost)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn cancel_job(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE jobs SET status = 'cancelled' WHERE id = ? AND status = 'pending'")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_job(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM jobs WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
