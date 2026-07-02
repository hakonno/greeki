use std::process::Stdio;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::process::Command;

use spotwatt_core::interval_cost;

use crate::model::{Job, JobStatus};
use crate::{db, AppState};

const MAX_OUTPUT_BYTES: usize = 8_000;

/// Run a job's shell command to completion and record the result. The job is
/// assumed to already be marked `running` (the scheduler claims it first).
pub async fn run_job(state: Arc<AppState>, job: Job, started: DateTime<Utc>) {
    let result = Command::new("sh")
        .arg("-c")
        .arg(&job.command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    let finished = Utc::now();

    let (status, exit_code, mut output) = match result {
        Ok(out) => {
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
        Err(e) => (JobStatus::Failed, None, format!("failed to start command: {e}")),
    };

    clip(&mut output, MAX_OUTPUT_BYTES);

    // Estimate what the run actually cost, using the price curve over the
    // real start/end window.
    let est_cost = {
        let prices = state.prices.read().await;
        job.power_kw()
            .and_then(|kw| interval_cost(&prices, started, finished, kw))
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
