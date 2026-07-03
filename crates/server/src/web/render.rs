//! HTML generation for the dashboard: page layout, the price horizon, job
//! cards, and the add-job form. Handlers live in the parent module.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Timelike, Utc};
use chrono_tz::Europe::Oslo;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use spotwatt_core::{
    cheapest_window, interval_cost, plan, Estimate, JobSpec, Policy, PriceSeries, Priority, Window,
};

use crate::config::Config;
use crate::model::{Job, JobStatus, Repeat};

use super::style::CSS;

/// What's already running, for explaining why a ready job hasn't launched.
struct Load {
    running: usize,
    max_jobs: usize,
    committed_watts: f64,
    budget_watts: Option<f64>,
}

impl Load {
    /// Why `job` can't be handed a slot right now, if anything blocks it.
    fn blocked_reason(&self, job: &Job) -> Option<String> {
        if self.running >= self.max_jobs {
            return Some(format!(
                "ready — waiting for a free slot ({} of {} running)",
                self.running, self.max_jobs
            ));
        }
        if let Some(budget) = self.budget_watts {
            let draw = job.power_watts.unwrap_or(0.0).max(0.0);
            if self.committed_watts + draw > budget {
                return Some(format!(
                    "ready — waiting for power-budget headroom ({:.0} of {:.0} W committed)",
                    self.committed_watts, budget
                ));
            }
        }
        None
    }
}

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

/// One column of the horizon chart: an hour bar, or the rule where the Oslo
/// date flips.
enum Col {
    Day(String),
    Bar {
        pct: f64,
        color: String,
        is_now: bool,
        tip: String,
        label: Option<String>,
    },
}

pub(super) fn render_prices(
    prices: &PriceSeries,
    raw_spot: &PriceSeries,
    vat_rate: f64,
    now: DateTime<Utc>,
) -> Markup {
    if prices.is_empty() {
        return html! { p.muted { "No price data yet — fetching…" } };
    }

    let current = prices.price_at(now);
    // The spot price incl. VAT — the number hvakosterstrommen.no shows, so
    // users can sanity-check us against it.
    let spot_mva = |t: DateTime<Utc>| {
        raw_spot
            .price_at(t)
            .map(|p| p.nok_per_kwh * (1.0 + vat_rate))
    };
    let min = prices.min_point();
    let max = prices.max_point();
    let avg = prices.avg_nok();

    // Show the current hour onward (the schedulable horizon). Stale data —
    // every known hour already past — would otherwise render an empty chart
    // with misleading stats from yesterday.
    let upcoming: Vec<_> = prices.points.iter().filter(|p| p.end > now).collect();
    if upcoming.is_empty() {
        return html! {
            p.muted { "Price data has gone stale — every known hour is in the past. Waiting for the next fetch…" }
        };
    }
    let hi = upcoming
        .iter()
        .map(|p| p.nok_per_kwh)
        .fold(0.0_f64, f64::max)
        .max(0.0001);
    let lo = upcoming
        .iter()
        .map(|p| p.nok_per_kwh)
        .fold(f64::INFINITY, f64::min)
        .min(hi);
    let span = (hi - lo).max(1e-9);
    let avg_pct = avg.map(|a| (a / hi * 100.0).clamp(0.0, 100.0));

    let win2 = cheapest_window(prices, now, None, 2);
    let win4 = cheapest_window(prices, now, None, 4);

    let mut cols = Vec::new();
    let mut prev_day = None;
    for p in &upcoming {
        let local = p.start.with_timezone(&Oslo);
        let day = local.date_naive();
        if prev_day.is_some_and(|d| d != day) {
            cols.push(Col::Day(local.format("%a").to_string().to_lowercase()));
        }
        prev_day = Some(day);

        // Color encodes where this hour sits between the horizon's cheapest
        // (green) and dearest (red) hour; height encodes the absolute price.
        let t = ((p.nok_per_kwh - lo) / span).clamp(0.0, 1.0);
        let color = format!(
            "hsl({:.0} {:.0}% {:.0}%)",
            145.0 - 137.0 * t,
            42.0 + 16.0 * t,
            46.0 + 8.0 * t
        );
        let is_now = p.contains(now);
        let tip = match spot_mva(p.start) {
            Some(s) => format!(
                "{} – {:.2} kr effective · spot {:.2} kr m/mva",
                fmt_oslo_hm(p.start),
                p.nok_per_kwh,
                s
            ),
            None => format!("{} – {:.2} kr effective", fmt_oslo_hm(p.start), p.nok_per_kwh),
        };
        // The "now" label overflows its slot, so keep interval labels a
        // couple of bars away from it.
        let label = if is_now {
            Some("now".to_string())
        } else if local.hour() % 3 == 0 && (p.start - now).num_hours() > 1 {
            Some(local.format("%H").to_string())
        } else {
            None
        };
        cols.push(Col::Bar {
            pct: (p.nok_per_kwh / hi * 100.0).max(2.0),
            color,
            is_now,
            tip,
            label,
        });
    }

    html! {
        div.readout {
            @if let Some(c) = current {
                div.now-price {
                    span.label { "effective now" }
                    span.value { (format!("{:.2}", c.nok_per_kwh)) small { " kr/kWh" } }
                }
            }
            div.summary {
                @if let Some(s) = spot_mva(now) {
                    div.stat { span.label { "spot m/mva" } span.value { (fmt_kr(s)) } }
                }
                @if let Some(a) = avg {
                    div.stat { span.label { "avg" } span.value { (fmt_kr(a)) } }
                }
                @if let Some(m) = min {
                    div.stat.good {
                        span.label { "min" }
                        span.value { (fmt_kr(m.nok_per_kwh)) small { " · " (fmt_oslo_hm(m.start)) } }
                    }
                }
                @if let Some(m) = max {
                    div.stat.bad {
                        span.label { "max" }
                        span.value { (fmt_kr(m.nok_per_kwh)) small { " · " (fmt_oslo_hm(m.start)) } }
                    }
                }
            }
        }
        div.chart {
            div.plot {
                div.bars {
                    @for col in &cols {
                        @match col {
                            Col::Day(d) => { div.sep { b { (d) } } }
                            Col::Bar { pct, color, is_now, tip, .. } => {
                                div.bar.now[*is_now] title=(tip)
                                    style=(format!("height:{pct:.1}%;background:{color}")) {}
                            }
                        }
                    }
                    @if let Some(p) = avg_pct {
                        div.avg-line style=(format!("bottom:{p:.1}%")) {}
                    }
                }
            }
            div.axis {
                @for col in &cols {
                    @match col {
                        Col::Day(_) => { div.sp {} }
                        Col::Bar { is_now, label, .. } => {
                            div.h.now[*is_now] {
                                @if let Some(l) = label { (l) }
                            }
                        }
                    }
                }
            }
        }
        div.legend {
            span { span.scale {} "cheap → expensive" }
            span.now-key { "▾ now" }
            span { "┄ avg" }
            span { "horizon " (prices.len()) " h" }
        }
        @if win2.is_some() || win4.is_some() {
            div.windows {
                @if let Some(w) = &win2 {
                    span.win { "cheapest 2 h " b { (fmt_window_range(w)) } (format!(" · avg {:.2} kr/kWh", w.avg_nok)) }
                }
                @if let Some(w) = &win4 {
                    span.win { "cheapest 4 h " b { (fmt_window_range(w)) } (format!(" · avg {:.2} kr/kWh", w.avg_nok)) }
                }
            }
        }
        p.footnote {
            "Bars show the effective price: spot + nettleie + elavgift + mva − strømstøtte. "
            "“Spot m/mva” is the number "
            a href="https://www.hvakosterstrommen.no" { "hvakosterstrommen.no" }
            " shows."
        }
    }
}

/// "Fri 03 Jul 15:00–17:00"
fn fmt_window_range(w: &Window) -> String {
    format!("{}–{}", fmt_oslo(w.start), fmt_oslo_hm(w.end))
}

pub(super) fn render_jobs(
    jobs: &[Job],
    learned: &HashMap<i64, Estimate>,
    savings: Option<(f64, i64)>,
    prices: &PriceSeries,
    config: &Config,
    now: DateTime<Utc>,
) -> Markup {
    if jobs.is_empty() {
        return html! {
            p.empty { "No jobs yet. Open “new job” above — the scheduler waits for cheap hours on its own." }
        };
    }
    let running: Vec<&Job> = jobs.iter().filter(|j| j.status == JobStatus::Running).collect();
    let load = Load {
        running: running.len(),
        max_jobs: config.max_concurrent_jobs,
        committed_watts: running.iter().filter_map(|j| j.power_watts).sum(),
        budget_watts: config.max_power_watts,
    };
    // Active work first: running, then pending, then history (newest first
    // within each group, which is the DB order).
    let mut ordered: Vec<&Job> = jobs.iter().collect();
    ordered.sort_by_key(|j| match j.status {
        JobStatus::Running => 0,
        JobStatus::Pending => 1,
        _ => 2,
    });
    html! {
        @if let Some((saved, n)) = savings {
            @if n > 0 {
                p.rollup {
                    "saved " b { (fmt_kr(saved)) } " vs running each job on submit · "
                    (n) " priced run" @if n != 1 { "s" } " · measured against the effective tariff"
                }
            }
        }
        div.jobs {
            @for job in &ordered {
                (render_job(job, learned.get(&job.id).copied(), prices, &load, now))
            }
        }
    }
}

fn render_job(
    job: &Job,
    learned: Option<Estimate>,
    prices: &PriceSeries,
    load: &Load,
    now: DateTime<Utc>,
) -> Markup {
    // Plan with the measured runtime when we have enough history, otherwise the
    // user's estimate.
    let eff_minutes = learned.map(|e| e.minutes).unwrap_or(job.duration_minutes);
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
                span class=(format!("status {}", status_class(job.status))) { (job.status.as_str()) }
                span.policy { (policy_label(&job.policy)) }
                @if job.priority != Priority::Normal {
                    span class=(format!("prio {}", priority_label(job.priority))) {
                        (priority_label(job.priority))
                    }
                }
                @if job.repeat == Repeat::Daily {
                    span.repeat { "↻ daily" }
                }
            }
            div.cmd { code { (job.command) } }

            div.meta {
                @if job.status == JobStatus::Pending || job.status == JobStatus::Running {
                    span.kv {
                        span.k { "est" } (dur_label(job.duration_minutes))
                        @if let Some(e) = learned {
                            @if e.minutes != job.duration_minutes {
                                span.learned {
                                    " · measured ~" (dur_label(e.minutes)) " over " (e.runs)
                                    @if e.exact { " runs" } @else { " similar runs" }
                                }
                            }
                        }
                    }
                }
                @if let Some(w) = job.power_watts {
                    span.kv { span.k { "draw" } (format!("{:.0} W", w)) }
                }
                @if let Some(e) = job.earliest_start {
                    @if e > now { span.kv { span.k { "not before" } (fmt_oslo(e)) } }
                }
                @if let Some(dl) = job.deadline {
                    span.kv { span.k { "finish by" } (fmt_oslo(dl)) }
                }
                @if job.status == JobStatus::Running {
                    @if let Some(f) = finish {
                        span.kv { span.k { "done" } "~" (fmt_oslo(f)) }
                    }
                }
            }

            @if let Some(d) = &decision {
                // The planner's reason assumes launch is instant; when the
                // slot count or power budget is what's actually holding the
                // job back, say that instead.
                @let queued = if d.run_now { load.blocked_reason(job) } else { None };
                div.plan {
                    @match &queued {
                        Some(q) => span.reason.queued { (q) },
                        None => span.reason { (d.reason) },
                    }
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
                        span.muted { "est. cost " (fmt_kr(wcost)) }
                    }
                }
            }

            @if job.status == JobStatus::Completed || job.status == JobStatus::Failed {
                div.result {
                    @if let Some(c) = job.exit_code {
                        span class=(if c == 0 { "kv" } else { "kv bad" }) {
                            span.k { "exit" } (c)
                        }
                    }
                    @if let Some(secs) = elapsed_secs(job) {
                        span.kv { span.k { "took" } (elapsed_label(secs)) }
                    }
                    @if let Some(s) = job.started_at {
                        span.kv { span.k { "ran" } (fmt_oslo(s)) }
                    }
                    @if let Some(cost) = job.est_cost_nok {
                        span.kv { span.k { "cost" } (fmt_kr(cost)) }
                    }
                    @if let (Some(c), Some(b)) = (job.est_cost_nok, job.baseline_cost_nok) {
                        @if b - c > 0.005 {
                            span.kv.good { "saved ~" (fmt_kr(b - c)) " vs submit" }
                        }
                    }
                }
                @if let Some(out) = &job.output {
                    @if !out.trim().is_empty() {
                        // hx-preserve keeps the fold open across the 5 s poll.
                        details.out id=(format!("out-{}", job.id)) hx-preserve="true" {
                            summary { "output" }
                            pre { (out.as_str()) }
                        }
                    }
                }
            }

            div.actions {
                @if job.status == JobStatus::Pending {
                    button.now hx-post=(format!("/jobs/{}/run", job.id)) hx-target="#jobs" hx-swap="innerHTML"
                        hx-confirm="Run now at the current price, skipping the schedule?" { "run now" }
                    button hx-post=(format!("/jobs/{}/cancel", job.id)) hx-target="#jobs" hx-swap="innerHTML" { "cancel" }
                }
                button.danger hx-post=(format!("/jobs/{}/delete", job.id)) hx-target="#jobs" hx-swap="innerHTML"
                    hx-confirm="Delete this job?" { "delete" }
            }
        }
    }
}

/// The collapsible "new job" panel. `open` expands it (used when there are no
/// jobs yet); `oob` marks it as an out-of-band swap so a successful create
/// replaces the form with a fresh, collapsed one.
pub(super) fn add_job_panel(open: bool, oob: bool) -> Markup {
    html! {
        details.adder #add-job open[open] hx-swap-oob=[oob.then_some("true")] {
            summary { h2 { "new job" } }
            (add_form())
        }
    }
}

fn add_form() -> Markup {
    html! {
        form hx-post="/jobs" hx-target="#jobs" hx-swap="innerHTML" {
            label.wide { "Command"
                textarea name="command" rows="2" placeholder="restic backup /data" required
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
                        option value="immediate" { "Immediate (ignore price)" }
                    }
                    small.hint { "When to start, given the price." }
                }
                label.threshold-field { "Threshold (kr/kWh)"
                    input type="number" step="any" name="threshold_nok" placeholder="1.20";
                    small.hint { "Against the effective price in the chart, not raw spot." }
                }
                label { "Est. duration (min)"
                    input #duration-input type="number" name="duration_minutes" value="60" min="1";
                    small.hint { "First guess; refined from real run times after a few runs." }
                }
                label { "Power (W)"
                    input type="number" step="1" name="power_watts" placeholder="150";
                    small.hint { "Lets the planner price the run and honor the site power budget." }
                }
                label { "Finish by (optional)"
                    input type="datetime-local" name="deadline";
                    small.hint { "Oslo time. The job is started early enough to be DONE by then — not started at this time." }
                }
                label { "Priority"
                    select name="priority" {
                        option value="normal" { "Normal" }
                        option value="low" { "Low" }
                        option value="high" { "High" }
                        option value="critical" { "Critical" }
                    }
                    small.hint { "Who gets a slot when slots are scarce. Unrelated to price." }
                }
                label { "Repeat"
                    select name="repeat" {
                        option value="none" { "Once" }
                        option value="daily" { "Daily (rolls deadline +24 h)" }
                    }
                }
            }
            button type="submit" { "add job" }
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
