//! HTML generation for the dashboard: page layout, the price chart, job cards,
//! and the add-job form. Handlers live in the parent module.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Europe::Oslo;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use spotwatt_core::{interval_cost, plan, JobSpec, Policy, PriceSeries, Priority};

use crate::model::{Job, JobStatus, Repeat};

use super::style::CSS;

pub(super) fn layout(body: Markup) -> Markup {
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

pub(super) fn render_prices(prices: &PriceSeries, now: DateTime<Utc>) -> Markup {
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

pub(super) fn render_jobs(
    jobs: &[Job],
    learned: &HashMap<i64, (i64, usize)>,
    savings: Option<(f64, i64)>,
    prices: &PriceSeries,
    now: DateTime<Utc>,
) -> Markup {
    if jobs.is_empty() {
        return html! { p.muted { "No jobs yet. Add one above." } };
    }
    html! {
        @if let Some((saved, n)) = savings {
            @if n > 0 {
                p.rollup {
                    "💰 est. " (fmt_kr(saved)) " saved vs starting each job on submit, over "
                    (n) " priced run" @if n != 1 { "s" }
                    " — measured against the effective tariff, not raw spot"
                }
            }
        }
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
                    @if let (Some(c), Some(b)) = (job.est_cost_nok, job.baseline_cost_nok) {
                        @if b - c > 0.005 { span.good { "saved ~" (fmt_kr(b - c)) " vs submit" } }
                    }
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

pub(super) fn add_form() -> Markup {
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
// Formatting helpers
// ---------------------------------------------------------------------------

fn fmt_kr(v: f64) -> String {
    format!("{:.2} kr", v)
}

pub(super) fn dur_label(minutes: i64) -> String {
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
