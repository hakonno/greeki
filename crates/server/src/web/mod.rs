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
use spotwatt_core::{Policy, PriceSeries, Priority};

use crate::model::{Job, NewJob, Repeat};
use crate::{db, executor, AppState};

use render::{add_form, dur_label, layout, render_jobs, render_prices};

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
    let effective = state.prices.read().await.with_tariff(&state.config.tariff);
    let (jobs, learned, savings) = jobs_with_learning(&state).await;
    let now = Utc::now();

    layout(html! {
        header {
            h1 { "spotwatt" }
            p.tag { "power-price-aware compute scheduler · region " (state.config.region) }
        }

        section.card {
            h2 { "Power price (effective)" }
            div #prices hx-get="/fragment/prices" hx-trigger="every 30s" hx-swap="innerHTML" {
                (render_prices(&effective, now))
            }
        }

        section.card {
            h2 { "Add a job" }
            (add_form())
        }

        section.card {
            h2 { "Jobs" }
            div #jobs hx-get="/fragment/jobs" hx-trigger="every 5s" hx-swap="innerHTML" {
                (render_jobs(&jobs, &learned, savings, &effective, now))
            }
        }

        footer {
            p { "Prices from hvakosterstrommen.no · the API only knows ~24–48h ahead, so plans are re-evaluated every tick." }
        }
    })
}

async fn prices_fragment(State(state): State<Arc<AppState>>) -> Markup {
    let effective = state.prices.read().await.with_tariff(&state.config.tariff);
    render_prices(&effective, Utc::now())
}

async fn jobs_fragment(State(state): State<Arc<AppState>>) -> Markup {
    let effective = state.prices.read().await.with_tariff(&state.config.tariff);
    let (jobs, learned, savings) = jobs_with_learning(&state).await;
    render_jobs(&jobs, &learned, savings, &effective, Utc::now())
}

/// Load all jobs together with each job's learned runtime (when it has enough
/// history, keyed by job id) and the all-time savings rollup.
async fn jobs_with_learning(
    state: &AppState,
) -> (Vec<Job>, HashMap<i64, (i64, usize)>, Option<(f64, i64)>) {
    let jobs = db::list_jobs(&state.db).await.unwrap_or_default();
    let mut learned = HashMap::new();
    for job in &jobs {
        if let Some(info) = db::learned_duration(&state.db, &job.command).await {
            learned.insert(job.id, info);
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
    let name = form.name.trim().to_string();
    let command = form.command.trim().to_string();

    if !name.is_empty() && !command.is_empty() {
        let policy = match form.policy.as_str() {
            "immediate" => Policy::Immediate,
            "threshold" => Policy::Threshold {
                max_nok_per_kwh: parse_f64(&form.threshold_nok).unwrap_or(0.5),
            },
            _ => Policy::CheapestWindow,
        };
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
        let new = NewJob {
            name,
            command,
            policy,
            duration_minutes: parse_i64(&form.duration_minutes).unwrap_or(60).max(1),
            deadline: form.deadline.as_deref().and_then(parse_deadline),
            power_watts: parse_f64(&form.power_watts),
            priority,
            repeat,
            earliest_start: None,
            created_at: Utc::now(),
        };
        if let Err(e) = db::create_job(&state.db, new).await {
            tracing::warn!("create job failed: {e:?}");
        }
    }

    let effective = state.prices.read().await.with_tariff(&state.config.tariff);
    let (jobs, learned, savings) = jobs_with_learning(&state).await;
    render_jobs(&jobs, &learned, savings, &effective, Utc::now())
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
    match db::learned_duration(&state.db, command).await {
        Some((est, n)) => html! {
            span.recognized {
                "✓ seen this command before — measured ~" (dur_label(est)) " over " (n)
                " runs (filled in below; the scheduler uses this)"
            }
            // Out-of-band: replace the duration input with the learned value.
            input #duration-input type="number" name="duration_minutes" min="1"
                value=(est) hx-swap-oob="true";
        },
        None => html! {},
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
