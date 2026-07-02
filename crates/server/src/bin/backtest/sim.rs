//! The simulation itself: replay each recurring demo job across the historical
//! price trace, three ways (typical / realistic foresight / oracle).

use chrono::{DateTime, Days, Duration, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Europe::Oslo;
use spotwatt_core::{cheapest_window, interval_cost, plan, slots_for, JobSpec, Policy, PriceSeries};

/// A representative recurring homelab job for the simulation.
pub struct DemoJob {
    pub name: &'static str,
    pub duration_minutes: i64,
    /// Oslo-local hour the job must finish by.
    pub deadline_hour: u32,
    /// Estimated average power draw, watts.
    pub power_watts: f64,
    /// How long before the deadline the job becomes available to run.
    pub window_hours: i64,
}

pub struct JobResult {
    pub typical: f64,
    pub spotwatt: f64,
    pub oracle: f64,
    pub runs: u32,
}

/// Simulate one recurring job across every eligible day in the range.
pub fn simulate_job(full: &PriceSeries, job: &DemoJob, start: NaiveDate, end: NaiveDate) -> JobResult {
    let dur = Duration::minutes(job.duration_minutes);
    let dur_h = job.duration_minutes as f64 / 60.0;
    let k = slots_for(job.duration_minutes);
    let power_kw = job.power_watts / 1000.0;

    let mut out = JobResult { typical: 0.0, spotwatt: 0.0, oracle: 0.0, runs: 0 };

    // Start at the second day so the 24h opportunity window has prior-day data.
    let mut d = match start.checked_add_days(Days::new(1)) {
        Some(d) => d,
        None => return out,
    };
    while d <= end {
        let deadline = oslo_dt(d, job.deadline_hour);
        let release = deadline - Duration::hours(job.window_hours);

        // "typical": expected cost at the window-average price.
        let typical = match avg_over(full, release, deadline) {
            Some(avg) => power_kw * dur_h * avg,
            None => {
                d = next(d);
                continue;
            }
        };

        // "oracle": cheapest window with full foresight.
        let oracle = match cheapest_window(full, release, Some(deadline), k)
            .and_then(|w| interval_cost(full, w.start, w.start + dur, power_kw))
        {
            Some(c) => c,
            None => {
                d = next(d);
                continue;
            }
        };

        // "spotwatt": realistic, limited-foresight start.
        let start_real = realistic_start(full, job, release, deadline);
        let spotwatt = match interval_cost(full, start_real, start_real + dur, power_kw) {
            Some(c) => c,
            None => {
                d = next(d);
                continue;
            }
        };

        out.typical += typical;
        out.oracle += oracle;
        out.spotwatt += spotwatt;
        out.runs += 1;
        d = next(d);
    }
    out
}

/// Replays the live scheduler's decision loop for one job instance: step hour by
/// hour, each time calling `plan` with only the prices that would have been
/// known at that moment, and return the hour it chooses to start.
fn realistic_start(
    full: &PriceSeries,
    job: &DemoJob,
    release: DateTime<Utc>,
    deadline: DateTime<Utc>,
) -> DateTime<Utc> {
    let spec = JobSpec {
        policy: Policy::CheapestWindow,
        duration_minutes: job.duration_minutes,
        deadline: Some(deadline),
        earliest_start: None,
    };
    let mut t = release;
    while t <= deadline {
        let known = truncate(full, known_horizon_end(t));
        let decision = plan(&spec, &known, t);
        if decision.run_now {
            return decision.start_at.unwrap_or(t);
        }
        t += Duration::hours(1);
    }
    // Should be unreachable: the deadline forces a start before we get here.
    deadline - Duration::minutes(job.duration_minutes)
}

/// End (exclusive) of the price horizon known at instant `t`: all of today's
/// Oslo-local prices, plus tomorrow's once it's past 13:00 local.
fn known_horizon_end(t: DateTime<Utc>) -> DateTime<Utc> {
    let local = t.with_timezone(&Oslo);
    let mut last = local.date_naive();
    if local.hour() >= 13 {
        last = next(last);
    }
    oslo_midnight_utc(next(last))
}

/// Prices restricted to those starting before `horizon_end` (the rest is
/// "unknown" at simulation time).
fn truncate(full: &PriceSeries, horizon_end: DateTime<Utc>) -> PriceSeries {
    PriceSeries {
        points: full
            .points
            .iter()
            .filter(|p| p.start < horizon_end)
            .cloned()
            .collect(),
    }
}

/// Mean NOK/kWh over the hours starting within `[from, until)`.
fn avg_over(full: &PriceSeries, from: DateTime<Utc>, until: DateTime<Utc>) -> Option<f64> {
    let vals: Vec<f64> = full
        .points
        .iter()
        .filter(|p| p.start >= from && p.start < until)
        .map(|p| p.nok_per_kwh)
        .collect();
    if vals.is_empty() {
        None
    } else {
        Some(vals.iter().sum::<f64>() / vals.len() as f64)
    }
}

// --- date/time helpers ----------------------------------------------------

pub fn next<T: NextDay>(d: T) -> T {
    d.next_day()
}

/// Tiny trait so `next()` works for both dates and is explicit about intent.
pub trait NextDay {
    fn next_day(self) -> Self;
}
impl NextDay for NaiveDate {
    fn next_day(self) -> Self {
        self.checked_add_days(Days::new(1)).expect("date overflow")
    }
}

fn oslo_dt(date: NaiveDate, hour: u32) -> DateTime<Utc> {
    let naive = date.and_hms_opt(hour, 0, 0).expect("valid hour");
    Oslo.from_local_datetime(&naive)
        .earliest()
        .expect("valid Oslo time")
        .with_timezone(&Utc)
}

fn oslo_midnight_utc(date: NaiveDate) -> DateTime<Utc> {
    oslo_dt(date, 0)
}
