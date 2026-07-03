//! The HTTP surface: routing, request handlers, and form parsing. All HTML
//! generation lives in [`render`]; the stylesheet in [`style`].

mod render;
mod style;

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Form, Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Europe::Oslo;
use maud::{html, Markup};
use serde::Deserialize;
use spotwatt_core::{Estimate, Policy, PriceSeries, Priority};

use crate::model::{Job, NewJob, Repeat};
use crate::{db, executor, AppState};

use render::{add_job_panel, dur_label, layout, render_jobs, render_prices};

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/jobs", post(create_job))
        .route("/jobs/:id/cancel", post(cancel))
        .route("/jobs/:id/run", post(run_now))
        .route("/jobs/:id/delete", post(delete))
        .route("/fragment/jobs", get(jobs_fragment))
        .route("/fragment/prices", get(prices_fragment))
        .route("/fragment/cmd-hint", get(cmd_hint))
        .route("/api/prices", get(api_prices))
        .route("/api/jobs", get(api_jobs))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Page handlers
// ---------------------------------------------------------------------------

async fn index(State(state): State<Arc<AppState>>) -> Markup {
    let raw = state.prices.read().await.clone();
    let effective = raw.with_tariff(&state.config.tariff);
    let (jobs, learned, savings) = jobs_with_learning(&state).await;
    let now = Utc::now();

    layout(html! {
        header {
            h1 { "spot" span.w { "watt" } }
            p.tag { "price-aware job scheduler · " (state.config.region) " · all times Oslo" }
        }

        section.card {
            h2 { "Power price" }
            div #prices hx-get="/fragment/prices" hx-trigger="every 30s" hx-swap="innerHTML" {
                (render_prices(&effective, &raw, state.config.tariff.vat_rate, now))
            }
        }

        section.card {
            (add_job_panel(jobs.is_empty(), false))
        }

        section.card {
            h2 { "Jobs" }
            div #jobs hx-get="/fragment/jobs" hx-trigger="every 5s" hx-swap="innerHTML" {
                (render_jobs(&jobs, &learned, savings, &effective, &state.config, now))
            }
        }

        footer {
            p { "prices from hvakosterstrommen.no · horizon ~24–48 h · plans re-evaluated every tick" }
        }
    })
}

async fn prices_fragment(State(state): State<Arc<AppState>>) -> Markup {
    let raw = state.prices.read().await.clone();
    let effective = raw.with_tariff(&state.config.tariff);
    render_prices(&effective, &raw, state.config.tariff.vat_rate, Utc::now())
}

async fn jobs_fragment(State(state): State<Arc<AppState>>) -> Markup {
    let effective = state.prices.read().await.with_tariff(&state.config.tariff);
    let (jobs, learned, savings) = jobs_with_learning(&state).await;
    render_jobs(&jobs, &learned, savings, &effective, &state.config, Utc::now())
}

/// Load all jobs together with each job's learned runtime (when it has enough
/// history, keyed by job id) and the all-time savings rollup.
async fn jobs_with_learning(
    state: &AppState,
) -> (Vec<Job>, HashMap<i64, Estimate>, Option<(f64, i64)>) {
    let jobs = db::list_jobs(&state.db).await.unwrap_or_default();
    let mut learned = HashMap::new();
    if let Ok(learner) = db::duration_learner(&state.db).await {
        for job in &jobs {
            if let Some(est) = learner.estimate(&job.command) {
                learned.insert(job.id, est);
            }
        }
    }
    let savings = db::savings_rollup(&state.db).await.ok();
    (jobs, learned, savings)
}

#[derive(Debug, Deserialize)]
pub struct CreateForm {
    name: String,
    command: String,
    policy: String,
    threshold_nok: Option<String>,
    duration_minutes: Option<String>,
    deadline: Option<String>,
    power_watts: Option<String>,
    priority: Option<String>,
    repeat: Option<String>,
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    Form(form): Form<CreateForm>,
) -> Markup {
    let errors = match validate(&form) {
        Ok(new) => {
            if let Err(e) = db::create_job(&state.db, new).await {
                tracing::warn!("create job failed: {e:?}");
                vec!["saving the job failed — see the server log".to_string()]
            } else {
                // Wake the scheduler so an immediately-runnable job starts in
                // milliseconds, not at the next tick.
                state.kick.notify_one();
                Vec::new()
            }
        }
        Err(errors) => errors,
    };

    let effective = state.prices.read().await.with_tariff(&state.config.tariff);
    let (jobs, learned, savings) = jobs_with_learning(&state).await;
    html! {
        @if !errors.is_empty() {
            div.errors role="alert" {
                "Job not created: " (errors.join(" · "))
            }
        } @else {
            // Out-of-band: replace the add-job panel with a fresh, collapsed
            // one so a successful create clears the form.
            (add_job_panel(false, true))
        }
        (render_jobs(&jobs, &learned, savings, &effective, &state.config, Utc::now()))
    }
}

/// Turn the raw form into a `NewJob`, or every reason it can't be one. The
/// old behavior — silently dropping the submission or papering over bad
/// values with defaults — meant a typo could quietly schedule the wrong job.
fn validate(form: &CreateForm) -> Result<NewJob, Vec<String>> {
    let mut errors = Vec::new();
    let name = form.name.trim().to_string();
    let command = form.command.trim().to_string();

    if name.is_empty() {
        errors.push("name is required".to_string());
    }
    if command.is_empty() {
        errors.push("command is required".to_string());
    }

    let policy = match form.policy.as_str() {
        "immediate" => Policy::Immediate,
        "threshold" => match parse_f64(&form.threshold_nok) {
            Some(max) if max > 0.0 => Policy::Threshold { max_nok_per_kwh: max },
            _ => {
                errors.push(
                    "the below-threshold policy needs a threshold (kr/kWh) above 0".to_string(),
                );
                Policy::CheapestWindow
            }
        },
        _ => Policy::CheapestWindow,
    };

    let duration_minutes = match (&form.duration_minutes, parse_i64(&form.duration_minutes)) {
        (_, Some(m)) if m >= 1 => m,
        (raw, _) if is_blank(raw) => 60,
        _ => {
            errors.push("duration must be a whole number of minutes ≥ 1".to_string());
            60
        }
    };

    let deadline = match (&form.deadline, form.deadline.as_deref().and_then(parse_deadline)) {
        (raw, None) if !is_blank(raw) => {
            errors.push("deadline could not be parsed".to_string());
            None
        }
        (_, Some(dl)) if dl <= Utc::now() => {
            errors.push("deadline is in the past".to_string());
            None
        }
        (_, dl) => dl,
    };

    let power_watts = match (&form.power_watts, parse_f64(&form.power_watts)) {
        (_, Some(w)) if w > 0.0 => Some(w),
        (raw, _) if is_blank(raw) => None,
        _ => {
            errors.push("power must be a number of watts above 0".to_string());
            None
        }
    };

    if !errors.is_empty() {
        return Err(errors);
    }

    let priority = match form.priority.as_deref().unwrap_or("normal") {
        "low" => Priority::Low,
        "high" => Priority::High,
        "critical" => Priority::Critical,
        _ => Priority::Normal,
    };
    let repeat = match form.repeat.as_deref().unwrap_or("none") {
        "daily" => Repeat::Daily,
        _ => Repeat::None,
    };
    Ok(NewJob {
        name,
        command,
        policy,
        duration_minutes,
        deadline,
        power_watts,
        priority,
        repeat,
        earliest_start: None,
        created_at: Utc::now(),
    })
}

fn is_blank(o: &Option<String>) -> bool {
    o.as_deref().map_or(true, |s| s.trim().is_empty())
}

async fn cancel(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> Markup {
    db::cancel_job(&state.db, id).await.ok();
    jobs_fragment(State(state)).await
}

async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> Markup {
    db::delete_job(&state.db, id).await.ok();
    jobs_fragment(State(state)).await
}

/// Manual override: start a pending job immediately, ignoring price and the
/// concurrency cap.
async fn run_now(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> Markup {
    if let Ok(Some(job)) = db::get_job(&state.db, id).await {
        let started = Utc::now();
        if let Ok(true) = db::claim_for_running(&state.db, id, started.timestamp()).await {
            let st = state.clone();
            tokio::spawn(async move { executor::run_job(st, job, started).await });
        }
    }
    jobs_fragment(State(state)).await
}

async fn api_prices(State(state): State<Arc<AppState>>) -> Json<PriceSeries> {
    Json(state.prices.read().await.clone())
}

async fn api_jobs(State(state): State<Arc<AppState>>) -> Json<Vec<Job>> {
    Json(db::list_jobs(&state.db).await.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
pub struct CmdQuery {
    command: Option<String>,
}

/// Live hint for the "add job" form: if the typed command has enough run
/// history, show what we measured and prefill the duration field (out-of-band).
async fn cmd_hint(State(state): State<Arc<AppState>>, Query(q): Query<CmdQuery>) -> Markup {
    let command = q.command.unwrap_or_default();
    let command = command.trim();
    if command.is_empty() {
        return html! {};
    }
    let est = match db::duration_learner(&state.db).await {
        Ok(learner) => learner.estimate(command),
        Err(_) => None,
    };
    let compound = spotwatt_core::has_shell_operators(command);
    html! {
        @if compound {
            span.warnhint {
                "⚠ compound command (;, &, |, $()) — runs as one job under sh -c, and its "
                "duration is only learned from exact repeats of the whole line."
            }
        }
        @if let Some(e) = est {
            span.recognized {
                @if e.exact {
                    "✓ seen this command before — measured ~" (dur_label(e.minutes))
                    " over " (e.runs) " runs"
                } @else {
                    "≈ similar commands (“" (spotwatt_core::command_signature(command))
                    "”) measured ~" (dur_label(e.minutes)) " over " (e.runs) " runs"
                }
                " (filled in below; the scheduler uses this)"
            }
            // Out-of-band: replace the duration input with the learned value.
            input #duration-input type="number" name="duration_minutes" min="1"
                value=(e.minutes) hx-swap-oob="true";
        }
    }
}

// ---------------------------------------------------------------------------
// Form parsing
// ---------------------------------------------------------------------------

fn parse_f64(o: &Option<String>) -> Option<f64> {
    o.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

fn parse_i64(o: &Option<String>) -> Option<i64> {
    o.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

/// Parse an `<input type="datetime-local">` value (Oslo local time) to UTC.
fn parse_deadline(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    Oslo.from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod preview {
    use super::*;
    use chrono::{Duration, TimeZone};
    use spotwatt_core::PricePoint;

    use crate::config::Config;
    use crate::model::JobStatus;

    fn pt(start: DateTime<Utc>, nok: f64) -> PricePoint {
        PricePoint {
            start,
            end: start + Duration::hours(1),
            nok_per_kwh: nok,
            eur_per_kwh: nok / 11.5,
        }
    }

    fn job(id: i64, name: &str, cmd: &str, status: JobStatus, now: DateTime<Utc>) -> Job {
        Job {
            id,
            name: name.into(),
            command: cmd.into(),
            policy: Policy::CheapestWindow,
            duration_minutes: 60,
            deadline: None,
            power_watts: None,
            priority: Priority::Normal,
            repeat: Repeat::None,
            earliest_start: None,
            status,
            scheduled_start: None,
            created_at: now - Duration::hours(3),
            started_at: None,
            finished_at: None,
            exit_code: None,
            output: None,
            est_cost_nok: None,
            baseline_cost_nok: None,
        }
    }

    /// Not a real test: renders the dashboard with synthetic data so the
    /// design can be eyeballed without a running server. Only fires when
    /// SPOTWATT_PREVIEW_OUT names an output file.
    #[test]
    fn dump_dashboard() {
        let Ok(path) = std::env::var("SPOTWATT_PREVIEW_OUT") else { return };
        let now = Utc.with_ymd_and_hms(2026, 7, 3, 10, 30, 0).unwrap();

        // Two July days: cheap night, morning ramp, evening peak.
        let shape = [
            0.55, 0.50, 0.48, 0.47, 0.50, 0.62, 0.85, 1.10, 1.25, 1.05, 0.90, 0.82,
            0.78, 0.75, 0.72, 0.80, 0.95, 1.30, 1.60, 1.45, 1.15, 0.90, 0.70, 0.60,
        ];
        let base = Utc.with_ymd_and_hms(2026, 7, 2, 22, 0, 0).unwrap(); // Oslo midnight, Jul 3 (CEST)
        let mut pts = Vec::new();
        for d in 0..2 {
            for (h, v) in shape.iter().enumerate() {
                let f = if d == 1 { 0.92 } else { 1.0 };
                pts.push(pt(base + Duration::hours((d * 24 + h) as i64), v * f));
            }
        }
        let effective = PriceSeries::new(pts.clone());
        let raw = PriceSeries::new(
            pts.iter()
                .map(|p| PricePoint { nok_per_kwh: (p.nok_per_kwh / 1.25) - 0.30, ..p.clone() })
                .collect(),
        );

        let mut transcode = job(1, "transcode queue", "ffmpeg -i /media/in.mkv -c:v libx265 /media/out.mkv", JobStatus::Running, now);
        transcode.started_at = Some(now - Duration::minutes(25));
        transcode.duration_minutes = 90;
        transcode.power_watts = Some(450.0);

        let mut backup = job(2, "nightly backup", "restic backup /data", JobStatus::Pending, now);
        backup.deadline = Some(Utc.with_ymd_and_hms(2026, 7, 4, 5, 0, 0).unwrap());
        backup.power_watts = Some(150.0);
        backup.repeat = Repeat::Daily;

        let mut scrub = job(3, "zfs scrub", "zpool scrub tank", JobStatus::Pending, now);
        scrub.policy = Policy::Threshold { max_nok_per_kwh: 1.00 };
        scrub.power_watts = Some(900.0);
        scrub.priority = Priority::Low;

        let mut done = job(4, "photo derivatives", "generate-thumbs --all /photos", JobStatus::Completed, now);
        done.started_at = Some(now - Duration::hours(9));
        done.finished_at = Some(now - Duration::hours(9) + Duration::minutes(38));
        done.exit_code = Some(0);
        done.est_cost_nok = Some(0.42);
        done.baseline_cost_nok = Some(0.97);
        done.output = Some("processed 1841 files\n0 errors".into());

        let mut failed = job(5, "model refresh", "python retrain.py --epochs 3", JobStatus::Failed, now);
        failed.started_at = Some(now - Duration::hours(20));
        failed.finished_at = Some(now - Duration::hours(20) + Duration::minutes(4));
        failed.exit_code = Some(2);
        failed.output = Some("Traceback (most recent call last):\n  File \"retrain.py\", line 12\nModuleNotFoundError: No module named 'torch'".into());

        let jobs = vec![backup.clone(), scrub, failed, done, transcode];
        let mut learned = HashMap::new();
        learned.insert(2_i64, Estimate { minutes: 42, runs: 7, exact: true });

        let mut config = Config::default();
        config.max_concurrent_jobs = 2;
        config.max_power_watts = Some(1000.0);

        let markup = layout(html! {
            header {
                h1 { "spot" span.w { "watt" } }
                p.tag { "price-aware job scheduler · NO3 · all times Oslo" }
            }
            section.card {
                h2 { "Power price" }
                div #prices { (render_prices(&effective, &raw, 0.25, now)) }
            }
            section.card { (add_job_panel(false, false)) }
            section.card {
                h2 { "Jobs" }
                div #jobs { (render_jobs(&jobs, &learned, Some((14.73, 12)), &effective, &config, now)) }
            }
            footer {
                p { "prices from hvakosterstrommen.no · horizon ~24–48 h · plans re-evaluated every tick" }
            }
        });
        std::fs::write(&path, markup.into_string()).unwrap();
        eprintln!("wrote {path}");
    }
}
