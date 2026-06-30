use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use spotwatt_core::{plan, select_within_budget};

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
    // Plan and cost against the effective consumer price (spot + grid + tax +
    // VAT − strømstøtte), not raw spot — that's the number the bill is in.
    let prices = state
        .prices
        .read()
        .await
        .with_tariff(&state.config.tariff);
    let now = Utc::now();

    // If the curve doesn't cover the current hour the data is stale or missing;
    // price-driven policies will correctly decline to start (only Immediate and
    // genuinely deadline-forced jobs run), but it's worth flagging.
    if !prices.is_empty() && prices.price_at(now).is_none() {
        tracing::warn!("price curve does not cover the current hour — prices may be stale");
    }

    let pending = db::pending_jobs(&state.db).await?;
    let mut runnable: Vec<Job> = Vec::new();

    for job in pending {
        let decision = plan(&job.spec(), &prices, now);
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

    // Two constraints: the job-count cap, and (if set) a site power budget that
    // keeps simultaneous draw down to shave the capacity tariff.
    let running = db::running_count(&state.db).await?;
    let free_slots = (state.config.max_concurrent_jobs as i64 - running).max(0) as usize;
    let committed_watts = db::running_power_watts(&state.db).await?;
    let candidate_watts: Vec<f64> = runnable
        .iter()
        .map(|j| j.power_watts.unwrap_or(0.0).max(0.0))
        .collect();
    let chosen = select_within_budget(
        &candidate_watts,
        free_slots,
        committed_watts,
        state.config.max_power_watts,
    );

    for idx in chosen {
        let job = runnable[idx].clone();
        // Claim the job atomically so a slow executor start can't cause a
        // second tick to launch the same job twice.
        let started = Utc::now();
        match db::claim_for_running(&state.db, job.id, started.timestamp()).await {
            Ok(true) => {
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
