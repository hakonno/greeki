//! The JSON API: a machine-readable surface alongside the HTML dashboard, for
//! scripts, the `spotwatt-cli` crate, or anything else that wants to submit
//! and watch jobs without going through the form/htmx flow.
//!
//! Every response is JSON. Errors use the matching HTTP status (`400` for a
//! request that fails validation, `404` for an unknown job id, `500` for a
//! storage failure) with a body of `{"errors": ["..."]}`.
//!
//! There is still no authentication here — see the README's security note.
//! Anyone who can reach this port can submit and run arbitrary shell commands
//! via this API exactly as they could via the HTML form.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use spotwatt_core::{Policy, PriceSeries, Priority};

use crate::model::{Job, NewJob, Repeat};
use crate::{db, executor, AppState};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/prices", get(prices))
        .route("/api/jobs", get(list_jobs).post(create_job))
        .route("/api/jobs/:id", get(get_job).delete(delete_job))
        .route("/api/jobs/:id/cancel", post(cancel_job))
        .route("/api/jobs/:id/run", post(run_job_now))
}

#[derive(Debug, Serialize)]
struct ApiError {
    errors: Vec<String>,
}

fn bad_request(errors: Vec<String>) -> Response {
    (StatusCode::BAD_REQUEST, Json(ApiError { errors })).into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError {
            errors: vec!["no job with that id".to_string()],
        }),
    )
        .into_response()
}

fn storage_error(context: &str, e: anyhow::Error) -> Response {
    tracing::warn!("{context}: {e:?}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            errors: vec![format!("{context} — see the server log")],
        }),
    )
        .into_response()
}

/// The current price curve, raw spot (kr/kWh, ex-VAT).
async fn prices(State(state): State<Arc<AppState>>) -> Json<PriceSeries> {
    Json(state.prices.read().await.clone())
}

async fn list_jobs(State(state): State<Arc<AppState>>) -> Json<Vec<Job>> {
    Json(db::list_jobs(&state.db).await.unwrap_or_default())
}

async fn get_job(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> Response {
    match db::get_job(&state.db, id).await {
        Ok(Some(job)) => Json(job).into_response(),
        Ok(None) => not_found(),
        Err(e) => storage_error("loading the job failed", e),
    }
}

/// Body for `POST /api/jobs`. Mirrors the HTML form's fields, but typed —
/// `policy` is tagged JSON (`{"kind":"threshold","max_nok_per_kwh":0.3}` or
/// `{"kind":"cheapest_window"}` / `{"kind":"immediate"}`) rather than a
/// threshold string parsed out of a form field, and `deadline` is RFC 3339
/// rather than a browser datetime-local string.
#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub policy: Policy,
    /// Whole minutes, defaults to 60 when omitted.
    pub duration_minutes: Option<i64>,
    /// RFC 3339, e.g. `"2026-09-05T07:00:00+02:00"`.
    pub deadline: Option<DateTime<Utc>>,
    pub power_watts: Option<f64>,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub repeat: Repeat,
}

/// Same checks as the HTML form's `validate`, just against already-typed
/// fields instead of raw strings — see `web::validate` for the sibling.
fn validate(req: CreateJobRequest) -> Result<NewJob, Vec<String>> {
    let mut errors = Vec::new();

    let name = req.name.trim().to_string();
    if name.is_empty() {
        errors.push("name is required".to_string());
    }
    let command = req.command.trim().to_string();
    if command.is_empty() {
        errors.push("command is required".to_string());
    }

    if let Policy::Threshold { max_nok_per_kwh } = req.policy {
        if max_nok_per_kwh.is_nan() || max_nok_per_kwh <= 0.0 {
            errors
                .push("the below-threshold policy needs a threshold (kr/kWh) above 0".to_string());
        }
    }

    let duration_minutes = match req.duration_minutes {
        None => 60,
        Some(m) if m >= 1 => m,
        Some(_) => {
            errors.push("duration must be a whole number of minutes ≥ 1".to_string());
            60
        }
    };

    if let Some(dl) = req.deadline {
        if dl <= Utc::now() {
            errors.push("deadline is in the past".to_string());
        }
    }

    if let Some(w) = req.power_watts {
        if w.is_nan() || w <= 0.0 {
            errors.push("power must be a number of watts above 0".to_string());
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(NewJob {
        name,
        command,
        policy: req.policy,
        duration_minutes,
        deadline: req.deadline,
        power_watts: req.power_watts,
        priority: req.priority,
        repeat: req.repeat,
        earliest_start: None,
        created_at: Utc::now(),
    })
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateJobRequest>,
) -> Response {
    let new = match validate(req) {
        Ok(new) => new,
        Err(errors) => return bad_request(errors),
    };
    match db::create_job(&state.db, new).await {
        Ok(job) => {
            // Wake the scheduler so an immediately-runnable job starts in
            // milliseconds, not at the next tick.
            state.kick.notify_one();
            (StatusCode::CREATED, Json(job)).into_response()
        }
        Err(e) => storage_error("saving the job failed", e),
    }
}

async fn cancel_job(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> Response {
    match db::get_job(&state.db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(e) => return storage_error("loading the job failed", e),
    }
    if let Err(e) = db::cancel_job(&state.db, id).await {
        return storage_error("cancelling the job failed", e);
    }
    get_job(State(state), Path(id)).await
}

async fn delete_job(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> Response {
    match db::get_job(&state.db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(e) => return storage_error("loading the job failed", e),
    }
    if let Err(e) = db::delete_job(&state.db, id).await {
        return storage_error("deleting the job failed", e);
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Manual override: start a pending job immediately, ignoring price and the
/// concurrency cap. Mirrors the dashboard's "run now" button.
async fn run_job_now(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> Response {
    let job = match db::get_job(&state.db, id).await {
        Ok(Some(job)) => job,
        Ok(None) => return not_found(),
        Err(e) => return storage_error("loading the job failed", e),
    };
    let started = Utc::now();
    match db::claim_for_running(&state.db, id, started.timestamp()).await {
        Ok(true) => {
            let st = state.clone();
            tokio::spawn(async move { executor::run_job(st, job, started).await });
            get_job(State(state), Path(id)).await
        }
        Ok(false) => bad_request(vec![
            "job is not pending — it may already be running or finished".to_string(),
        ]),
        Err(e) => storage_error("starting the job failed", e),
    }
}
