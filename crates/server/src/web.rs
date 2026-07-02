use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Form, Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Europe::Oslo;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;
use spotwatt_core::{interval_cost, plan, JobSpec, Policy, PriceSeries, Priority};

use crate::model::{Job, JobStatus, NewJob, Repeat};
use crate::{db, executor, AppState};

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
    let (jobs, learned) = jobs_with_learning(&state).await;
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
                (render_jobs(&jobs, &learned, &effective, now))
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
    let (jobs, learned) = jobs_with_learning(&state).await;
    render_jobs(&jobs, &learned, &effective, Utc::now())
}

/// Load all jobs together with each job's learned runtime (when it has enough
/// history), keyed by job id.
async fn jobs_with_learning(state: &AppState) -> (Vec<Job>, HashMap<i64, (i64, usize)>) {
    let jobs = db::list_jobs(&state.db).await.unwrap_or_default();
    let mut learned = HashMap::new();
    for job in &jobs {
        if let Some(info) = db::learned_duration(&state.db, &job.command).await {
            learned.insert(job.id, info);
        }
    }
    (jobs, learned)
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
    let (jobs, learned) = jobs_with_learning(&state).await;
    render_jobs(&jobs, &learned, &effective, Utc::now())
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
// Rendering
// ---------------------------------------------------------------------------

fn layout(body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "spotwatt" }
                script src="https://unpkg.com/htmx.org@1.9.12" {}
                style { (PreEscaped(CSS)) }
            }
            body { main { (body) } }
        }
    }
}

fn render_prices(prices: &PriceSeries, now: DateTime<Utc>) -> Markup {
    if prices.is_empty() {
        return html! { p.muted { "No price data yet — fetching…" } };
    }

    let current = prices.price_at(now);
    let min = prices.min_point();
    let max = prices.max_point();
    let avg = prices.avg_nok();
    let cheapest_start = min.map(|p| p.start);

    // Show the current hour onward (the schedulable horizon).
    let upcoming: Vec<_> = prices.points.iter().filter(|p| p.end > now).collect();
    let scale = upcoming
        .iter()
        .map(|p| p.nok_per_kwh)
        .fold(0.0_f64, f64::max)
        .max(0.0001);

    html! {
        div.summary {
            @if let Some(c) = current {
                div.stat { span.label { "now" } span.value { (fmt_kr(c.nok_per_kwh)) } }
            }
            @if let Some(a) = avg {
                div.stat { span.label { "avg" } span.value { (fmt_kr(a)) } }
            }
            @if let Some(m) = min {
                div.stat.good { span.label { "min" } span.value { (fmt_kr(m.nok_per_kwh)) } }
            }
            @if let Some(m) = max {
                div.stat.bad { span.label { "max" } span.value { (fmt_kr(m.nok_per_kwh)) } }
            }
        }
        div.chart {
            @for p in &upcoming {
                @let h = (p.nok_per_kwh / scale * 100.0).max(3.0);
                @let kind = if p.contains(now) { "now" } else if Some(p.start) == cheapest_start { "cheap" } else { "" };
                div.bar-wrap title=(format!("{} – {:.3} kr", fmt_oslo_hm(p.start), p.nok_per_kwh)) {
                    div class=(format!("bar {}", kind)) style=(format!("height:{:.1}%", h)) {}
                    span.hour { (p.start.with_timezone(&Oslo).format("%H").to_string()) }
                }
            }
        }
        p.muted { "Known horizon: " (prices.len()) " hours. 🟡 Yellow = current hour. 🟢 Green = cheapest. Effective price: spot + nettleie + elavgift + MVA − strømstøtte." }
    }
}

fn render_jobs(
    jobs: &[Job],
    learned: &HashMap<i64, (i64, usize)>,
    prices: &PriceSeries,
    now: DateTime<Utc>,
) -> Markup {
    if jobs.is_empty() {
        return html! { p.muted { "No jobs yet. Add one above." } };
    }
    html! {
        div.jobs {
            @for job in jobs {
                (render_job(job, learned.get(&job.id).copied(), prices, now))
            }
        }
    }
}

fn render_job(
    job: &Job,
    learned: Option<(i64, usize)>,
    prices: &PriceSeries,
    now: DateTime<Utc>,
) -> Markup {
    // Plan with the measured runtime when we have enough history, otherwise the
    // user's estimate.
    let eff_minutes = learned.map(|(est, _)| est).unwrap_or(job.duration_minutes);
    let spec = JobSpec {
        policy: job.policy,
        duration_minutes: eff_minutes,
        deadline: job.deadline,
        earliest_start: job.earliest_start,
    };
    let dur = Duration::minutes(eff_minutes.max(0));

    let decision = if job.status == JobStatus::Pending {
        Some(plan(&spec, prices, now))
    } else {
        None
    };

    // Estimated finish time, using the effective duration.
    let finish = match job.status {
        JobStatus::Running => job.started_at.map(|s| s + dur),
        JobStatus::Pending => decision.as_ref().and_then(|d| d.start_at).map(|s| s + dur),
        _ => None,
    };

    // Projected savings vs. running right now (only for deferrable jobs we can price).
    let projected = decision.as_ref().and_then(|d| {
        let kw = job.power_kw()?;
        let start = d.start_at?;
        let window = interval_cost(prices, start, start + dur, kw)?;
        let now_cost = interval_cost(prices, now, now + dur, kw)?;
        Some((now_cost - window, window))
    });

    html! {
        div.job {
            div.job-head {
                span.name { (job.name) }
                span class=(format!("badge {}", status_class(job.status))) { (job.status.as_str()) }
                span.policy { (policy_label(&job.policy)) }
                @if job.priority != Priority::Normal {
                    span.prio { (priority_label(job.priority)) }
                }
                @if job.repeat == Repeat::Daily {
                    span.prio { "↻ daily" }
                }
            }
            div.cmd { code { (job.command) } }

            div.meta {
                @if job.status == JobStatus::Pending || job.status == JobStatus::Running {
                    span {
                        "⏱ est. " (dur_label(job.duration_minutes))
                        @if let Some((est, n)) = learned {
                            @if est != job.duration_minutes {
                                span.learned { " (measured ~" (dur_label(est)) " over " (n) " runs)" }
                            }
                        }
                    }
                }
                @if let Some(w) = job.power_watts { span { "⚡ " (format!("{:.0}", w)) " W" } }
                @if let Some(e) = job.earliest_start {
                    @if e > now { span { "⏳ not before " (fmt_oslo(e)) } }
                }
                @if let Some(dl) = job.deadline { span { "⛳ finish by " (fmt_oslo(dl)) } }
                @if job.status == JobStatus::Running {
                    @if let Some(f) = finish { span { "≈ done " (fmt_oslo(f)) } }
                }
            }

            @if let Some(d) = &decision {
                div.plan {
                    span.reason { (d.reason) }
                    @if let Some(start) = d.start_at {
                        @if !d.run_now {
                            span.when { "→ starts " (fmt_oslo(start)) }
                        }
                    }
                    @if let Some(f) = finish {
                        span.muted { "done by ~" (fmt_oslo(f)) }
                    }
                    @if d.forced { span.warn { "forced" } }
                }
                @if let Some((save, wcost)) = projected {
                    div.savings {
                        @if save > 0.01 {
                            span.good { "saves ~" (fmt_kr(save)) " vs now" }
                        }
                        span.muted { "est. " (fmt_kr(wcost)) }
                    }
                }
            }

            @if job.status == JobStatus::Completed || job.status == JobStatus::Failed {
                div.result {
                    @if let Some(c) = job.exit_code { span { "exit " (c) } }
                    @if let Some(secs) = elapsed_secs(job) { span { "took " (elapsed_label(secs)) } }
                    @if let Some(s) = job.started_at { span { "ran " (fmt_oslo(s)) } }
                    @if let Some(cost) = job.est_cost_nok { span { "cost " (fmt_kr(cost)) } }
                }
                @if let Some(out) = &job.output {
                    @if !out.trim().is_empty() {
                        details { summary { "output" } pre { (out.as_str()) } }
                    }
                }
            }

            div.actions {
                @if job.status == JobStatus::Pending {
                    button hx-post=(format!("/jobs/{}/run", job.id)) hx-target="#jobs" hx-swap="innerHTML" { "run now" }
                    button hx-post=(format!("/jobs/{}/cancel", job.id)) hx-target="#jobs" hx-swap="innerHTML" { "cancel" }
                }
                button.danger hx-post=(format!("/jobs/{}/delete", job.id)) hx-target="#jobs" hx-swap="innerHTML"
                    hx-confirm="Delete this job?" { "delete" }
            }
        }
    }
}

fn add_form() -> Markup {
    html! {
        form hx-post="/jobs" hx-target="#jobs" hx-swap="innerHTML" {
            label.wide { "Command"
                textarea name="command" rows="2" placeholder="restic backup /data"
                    hx-get="/fragment/cmd-hint" hx-trigger="change, keyup changed delay:500ms"
                    hx-target="#cmd-hint" hx-swap="innerHTML" {}
                small #cmd-hint .hint {}
            }
            div.grid {
                label { "Name" input type="text" name="name" placeholder="nightly backup" required; }
                label { "Policy"
                    select name="policy" {
                        option value="cheapest" { "Cheapest window" }
                        option value="threshold" { "Below threshold" }
                        option value="immediate" { "Immediate (critical)" }
                    }
                }
                label { "Est. duration (min)"
                    input #duration-input type="number" name="duration_minutes" value="60" min="1";
                    small.hint { "First guess; refined from real run times after a few runs." }
                }
                label { "Threshold (kr/kWh)"
                    input type="number" step="0.01" name="threshold_nok" placeholder="1.20";
                    small.hint { "Compared against the effective price in the chart, not raw spot." }
                }
                label { "Finish by (optional)"
                    input type="datetime-local" name="deadline";
                    small.hint { "Job must be DONE by this time — it's started early enough to finish, not started at this time." }
                }
                label { "Power (W)" input type="number" step="1" name="power_watts" placeholder="150"; }
                label { "Priority"
                    select name="priority" {
                        option value="normal" { "Normal" }
                        option value="low" { "Low" }
                        option value="high" { "High" }
                        option value="critical" { "Critical" }
                    }
                }
                label { "Repeat"
                    select name="repeat" {
                        option value="none" { "Once" }
                        option value="daily" { "Daily (rolls deadline +24h)" }
                    }
                }
            }
            button type="submit" { "Add job" }
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn fmt_kr(v: f64) -> String {
    format!("{:.2} kr", v)
}

fn dur_label(minutes: i64) -> String {
    if minutes >= 60 && minutes % 60 == 0 {
        format!("{} h", minutes / 60)
    } else if minutes > 60 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

/// Actual wall-clock seconds a finished job took, if both timestamps exist.
fn elapsed_secs(job: &Job) -> Option<i64> {
    match (job.started_at, job.finished_at) {
        (Some(s), Some(f)) if f >= s => Some((f - s).num_seconds()),
        _ => None,
    }
}

/// Human-friendly elapsed time: seconds under a minute, then minutes, then hours.
fn elapsed_label(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let (m, s) = (secs / 60, secs % 60);
        if s == 0 {
            format!("{m} min")
        } else {
            format!("{m} min {s}s")
        }
    } else {
        let (h, m) = (secs / 3600, (secs % 3600) / 60);
        if m == 0 {
            format!("{h} h")
        } else {
            format!("{h} h {m} min")
        }
    }
}

fn fmt_oslo(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Oslo).format("%a %d %b %H:%M").to_string()
}

fn fmt_oslo_hm(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Oslo).format("%H:%M").to_string()
}

fn status_class(s: JobStatus) -> &'static str {
    match s {
        JobStatus::Pending => "pending",
        JobStatus::Running => "running",
        JobStatus::Completed => "completed",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
    }
}

fn policy_label(p: &Policy) -> String {
    match p {
        Policy::Immediate => "immediate".to_string(),
        Policy::Threshold { max_nok_per_kwh } => format!("≤ {:.2} kr", max_nok_per_kwh),
        Policy::CheapestWindow => "cheapest window".to_string(),
    }
}

fn priority_label(p: Priority) -> &'static str {
    match p {
        Priority::Low => "low",
        Priority::Normal => "normal",
        Priority::High => "high",
        Priority::Critical => "critical",
    }
}

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

const CSS: &str = r#"
:root{--bg:#0f1115;--card:#171a21;--line:#262b36;--fg:#e6e9ef;--muted:#8b93a3;
--accent:#f4b740;--good:#46c98b;--bad:#ef6f6f;--blue:#5b8def}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);
font:15px/1.5 ui-sans-serif,system-ui,-apple-system,Segoe UI,Roboto,sans-serif}
main{max-width:920px;margin:0 auto;padding:24px 16px 60px}
header h1{margin:0;font-size:26px;letter-spacing:.5px}
.tag{color:var(--muted);margin:.2em 0 1.4em}
.card{background:var(--card);border:1px solid var(--line);border-radius:12px;
padding:18px 20px;margin-bottom:20px}
.card h2{margin:0 0 14px;font-size:15px;text-transform:uppercase;letter-spacing:.08em;color:var(--muted)}
.summary{display:flex;gap:22px;flex-wrap:wrap;margin-bottom:14px}
.stat{display:flex;flex-direction:column}
.stat .label{font-size:11px;color:var(--muted);text-transform:uppercase}
.stat .value{font-size:20px;font-weight:600}
.stat.good .value{color:var(--good)} .stat.bad .value{color:var(--bad)}
.chart{display:flex;align-items:flex-end;gap:3px;height:140px;margin-top:8px}
.bar-wrap{flex:1;display:flex;flex-direction:column;justify-content:flex-end;align-items:center;height:100%}
.bar{width:100%;background:var(--blue);border-radius:3px 3px 0 0;min-height:3px;transition:height .3s}
.bar.now{background:var(--accent)} .bar.cheap{background:var(--good)}
.hour{font-size:9px;color:var(--muted);margin-top:3px}
.muted{color:var(--muted);font-size:13px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px;margin-bottom:14px}
label{display:flex;flex-direction:column;font-size:12px;color:var(--muted);gap:4px}
input,select,textarea{background:var(--bg);border:1px solid var(--line);color:var(--fg);
border-radius:8px;padding:8px 10px;font-size:14px}
textarea{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;resize:vertical;min-height:42px;width:100%}
.wide{display:flex;flex-direction:column;gap:4px;margin-bottom:12px}
.hint{display:block;font-size:11px;color:var(--muted);margin-top:3px;font-weight:400}
.learned{color:var(--good)}
.recognized{color:var(--good)}
button{background:var(--blue);color:#fff;border:0;border-radius:8px;padding:8px 14px;
font-size:13px;cursor:pointer}
button:hover{filter:brightness(1.1)}
button.danger{background:transparent;border:1px solid var(--bad);color:var(--bad)}
.jobs{display:flex;flex-direction:column;gap:12px}
.job{border:1px solid var(--line);border-radius:10px;padding:14px 16px;background:#12151c}
.job-head{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.job-head .name{font-weight:600;font-size:16px}
.badge{font-size:11px;padding:2px 8px;border-radius:20px;text-transform:uppercase;letter-spacing:.05em}
.badge.pending{background:#2a2f3a;color:var(--muted)}
.badge.running{background:#36506f;color:#cfe2ff}
.badge.completed{background:#1f4636;color:#9fe6c2}
.badge.failed{background:#4a2330;color:#ffb3b3}
.badge.cancelled{background:#33363f;color:#9aa2b1}
.policy,.prio{font-size:12px;color:var(--muted)}
.prio{color:var(--accent)}
.cmd{margin:8px 0}
.cmd code{background:#0b0d12;border:1px solid var(--line);border-radius:6px;
padding:3px 8px;font-size:13px;display:inline-block;color:#cdd6e3}
.meta{display:flex;gap:16px;flex-wrap:wrap;color:var(--muted);font-size:13px;margin-bottom:6px}
.plan{display:flex;gap:12px;align-items:center;flex-wrap:wrap;font-size:13px;margin-top:4px}
.plan .reason{color:#cdd6e3}
.plan .when{color:var(--blue)}
.plan .warn{color:var(--accent);font-weight:600}
.savings{display:flex;gap:14px;font-size:13px;margin-top:4px}
.savings .good{color:var(--good)}
.result{display:flex;gap:16px;color:var(--muted);font-size:13px;margin-top:6px}
details{margin-top:8px} summary{cursor:pointer;color:var(--muted);font-size:13px}
pre{background:#0b0d12;border:1px solid var(--line);border-radius:6px;
padding:10px;overflow:auto;font-size:12px;max-height:240px}
.actions{display:flex;gap:8px;margin-top:12px}
.actions button{font-size:12px;padding:6px 12px}
footer{color:var(--muted);font-size:12px;margin-top:10px;text-align:center}
"#;
