use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use spotwatt_core::{plan, JobSpec};

use crate::model::Job;
use crate::{db, executor, AppState};

/// Re-evaluate all pending jobs on a fixed interval. Each tick is stateless:
/// it asks the core planner what each job should do *right now* given the
/// latest prices, persists the projected start time for display, and launches
/// any job that should run — subject to the concurrency cap.
pub async fn run(state: Arc<AppState>) {
    let period = Duration::from_secs(state.config.tick_seconds.max(5));
    let mut ticker = tokio::time::interval(period);
    loop {
        ticker.tick().await;
        if let Err(e) = tick(&state).await {
            tracing::warn!("scheduler tick failed: {e:?}");
        }
    }
}

async fn tick(state: &Arc<AppState>) -> Result<()> {
    let prices = state.prices.read().await.clone();
    let now = Utc::now();

    let pending = db::pending_jobs(&state.db).await?;
    let mut runnable: Vec<Job> = Vec::new();

    for job in pending {
        // Plan with the learned runtime when we have enough history, otherwise
        // the user's estimate.
        let duration = db::learned_duration(&state.db, &job.command)
            .await
            .map(|(est, _)| est)
            .unwrap_or(job.duration_minutes);
        let spec = JobSpec {
            policy: job.policy,
            duration_minutes: duration,
            deadline: job.deadline,
        };
        let decision = plan(&spec, &prices, now);
        db::set_scheduled_start(&state.db, job.id, decision.start_at.map(|d| d.timestamp()))
            .await
            .ok();
        if decision.run_now {
            runnable.push(job);
        }
    }

    if runnable.is_empty() {
        return Ok(());
    }

    // Highest priority first, then earliest deadline first.
    runnable.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| match (a.deadline, b.deadline) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            })
    });

    let running = db::running_count(&state.db).await?;
    let mut free = (state.config.max_concurrent_jobs as i64 - running).max(0);

    for job in runnable {
        if free <= 0 {
            break;
        }
        // Claim the job atomically so a slow executor start can't cause a
        // second tick to launch the same job twice.
        let started = Utc::now();
        match db::claim_for_running(&state.db, job.id, started.timestamp()).await {
            Ok(true) => {
                free -= 1;
                let st = state.clone();
                tracing::info!("launching job {} ({})", job.id, job.name);
                tokio::spawn(async move { executor::run_job(st, job, started).await });
            }
            Ok(false) => {} // someone else claimed it
            Err(e) => tracing::warn!("failed to claim job {}: {e:?}", job.id),
        }
    }

    Ok(())
}
