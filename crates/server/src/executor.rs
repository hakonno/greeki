use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::process::Command;

use spotwatt_core::interval_cost;

use crate::model::{Job, JobStatus};
use crate::{db, AppState};

const MAX_OUTPUT_BYTES: usize = 8_000;

/// Run a job's shell command to completion and record the result. The job is
/// assumed to already be marked `running` (the scheduler claims it first).
pub async fn run_job(state: Arc<AppState>, job: Job, started: DateTime<Utc>) {
    let timeout = Duration::from_secs(state.config.job_timeout_minutes.max(1) * 60);

    // `kill_on_drop` means that if we abandon the wait future on timeout, the
    // shell is killed rather than leaked. That alone is not enough: `sh -c`
    // often spawns children of its own (pipelines, `&&` chains), and killing
    // only the shell would leave them running past the timeout. Putting the
    // shell in its own process group lets the timeout kill the whole tree.
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&job.command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    let (status, exit_code, mut output) = match cmd.spawn() {
        Err(e) => (JobStatus::Failed, None, format!("failed to start command: {e}")),
        Ok(child) => {
            let pgid = child.id();
            match tokio::time::timeout(timeout, child.wait_with_output()).await {
                Ok(Ok(out)) => {
                    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if !stderr.trim().is_empty() {
                        text.push_str("\n[stderr]\n");
                        text.push_str(&stderr);
                    }
                    let status = if out.status.success() {
                        JobStatus::Completed
                    } else {
                        JobStatus::Failed
                    };
                    (status, out.status.code().map(|c| c as i64), text)
                }
                Ok(Err(e)) => (JobStatus::Failed, None, format!("command error: {e}")),
                Err(_elapsed) => {
                    kill_group(pgid);
                    (
                        JobStatus::Failed,
                        None,
                        format!(
                            "killed: exceeded the {} min timeout",
                            state.config.job_timeout_minutes
                        ),
                    )
                }
            }
        }
    };

    let finished = Utc::now();
    clip(&mut output, MAX_OUTPUT_BYTES);

    // Estimate what the run actually cost — and what the same run would have
    // cost had it started the moment the job was submitted — both against the
    // *effective* consumer price. The baseline is what the savings report is
    // measured against; it is None (and the run doesn't count toward savings)
    // when the price curve no longer covers the submit time.
    let (est_cost, baseline_cost) = {
        let prices = state.prices.read().await.with_tariff(&state.config.tariff);
        match job.power_kw() {
            Some(kw) => (
                interval_cost(&prices, started, finished, kw),
                interval_cost(
                    &prices,
                    job.created_at,
                    job.created_at + (finished - started),
                    kw,
                ),
            ),
            None => (None, None),
        }
    };

    if let Err(e) = db::mark_finished(
        &state.db,
        job.id,
        status,
        finished.timestamp(),
        exit_code,
        &output,
        est_cost,
        baseline_cost,
    )
    .await
    {
        tracing::warn!("failed to record completion of job {}: {e:?}", job.id);
    }

    // Recurring jobs queue their next occurrence once this one is done, so a
    // "nightly backup before 07:00" is actually nightly.
    match db::create_next_occurrence(&state.db, &job).await {
        Ok(Some(next)) => tracing::info!("queued next occurrence of {} as job {}", job.name, next.id),
        Ok(None) => {}
        Err(e) => tracing::warn!("failed to queue next occurrence of job {}: {e:?}", job.id),
    }

    tracing::info!(
        "job {} ({}) finished: {} (exit {:?})",
        job.id,
        job.name,
        status.as_str(),
        exit_code
    );
}

/// Kill a timed-out job's whole process group. The shell was spawned as its
/// own group leader, so its pid doubles as the pgid and a negative pid signals
/// every process in the tree — including children the shell forked.
#[cfg(unix)]
fn kill_group(pgid: Option<u32>) {
    if let Some(pgid) = pgid {
        unsafe { libc::kill(-(pgid as i32), libc::SIGKILL) };
    }
}

#[cfg(not(unix))]
fn kill_group(_pgid: Option<u32>) {}

/// Truncate a string to at most `max` bytes without splitting a UTF-8 char.
fn clip(s: &mut String, max: usize) {
    if s.len() > max {
        let mut i = max;
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        s.truncate(i);
        s.push_str("\n…[truncated]");
    }
}
