use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::process::Command;

use crate::model::{Job, JobStatus};
use crate::{cost, db, AppState};

const MAX_OUTPUT_BYTES: usize = 8_000;

/// Run a job's shell command to completion and record the result. The job is
/// assumed to already be marked `running` (the scheduler claims it first).
pub async fn run_job(state: Arc<AppState>, job: Job, started: DateTime<Utc>) {
    let timeout = Duration::from_secs(state.config.job_timeout_minutes.max(1) * 60);

    // `kill_on_drop` means that if we abandon the wait future on timeout, the
    // child is killed rather than leaked.
    let child = Command::new("sh")
        .arg("-c")
        .arg(&job.command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();

    let (status, exit_code, mut output) = match child {
        Err(e) => (JobStatus::Failed, None, format!("failed to start command: {e}")),
        Ok(child) => match tokio::time::timeout(timeout, child.wait_with_output()).await {
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
            Err(_elapsed) => (
                JobStatus::Failed,
                None,
                format!(
                    "killed: exceeded the {} min timeout",
                    state.config.job_timeout_minutes
                ),
            ),
        },
    };

    let finished = Utc::now();
    clip(&mut output, MAX_OUTPUT_BYTES);

    // Estimate what the run actually cost, using the *effective* consumer price
    // (spot + grid + tax + VAT − strømstøtte) over the real start/end window.
    let est_cost = {
        let prices = state.prices.read().await.with_tariff(&state.config.tariff);
        job.power_kw()
            .and_then(|kw| cost::interval_cost(&prices, started, finished, kw))
    };

    if let Err(e) = db::mark_finished(
        &state.db,
        job.id,
        status,
        finished.timestamp(),
        exit_code,
        &output,
        est_cost,
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
