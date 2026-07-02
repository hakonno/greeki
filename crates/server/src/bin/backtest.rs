//! spotwatt backtester.
//!
//! Replays real historical spot prices through the same `core::plan()` the live
//! scheduler uses, and reports what each scheduling policy would have cost over
//! the period. Inspired by the "fast simulator" idea from HPC scheduling
//! research (Wilkinson et al.; Menear et al., SC'25): scheduling is
//! deterministic, so you can replay a historical trace cheaply and compare
//! strategies against a baseline.
//!
//! Three numbers per job:
//!   * typical  — expected cost running at no particular time (window-average
//!                price). This is "what you'd pay without spotwatt".
//!   * spotwatt — cheapest window, but only with the *realistic* 24–48h price
//!                foresight the live system actually has at each moment.
//!   * oracle   — cheapest window with perfect foresight (theoretical best).
//!
//! "capture" is how much of the achievable saving (typical → oracle) the
//! realistic scheduler actually captured.
//!
//! Run: `cargo run -p spotwatt-server --bin backtest -- --region NO1 --days 365`

use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Days, Duration, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Europe::Oslo;
use spotwatt_core::{
    cheapest_window, interval_cost, plan, slots_for, JobSpec, Policy, PricePoint, PriceSeries,
};
use strompris::{PriceRegion, Strompris};

/// A representative recurring homelab job for the simulation.
struct DemoJob {
    name: &'static str,
    duration_minutes: i64,
    /// Oslo-local hour the job must finish by.
    deadline_hour: u32,
    /// Estimated average power draw, watts.
    power_watts: f64,
    /// How long before the deadline the job becomes available to run.
    window_hours: i64,
}

struct JobResult {
    typical: f64,
    spotwatt: f64,
    oracle: f64,
    runs: u32,
}

#[tokio::main]
async fn main() {
    let (region, days) = parse_args();

    let today = Utc::now().with_timezone(&Oslo).date_naive();
    let start = today - Days::new(days);

    eprintln!("Loading prices for {region}: {start} … {today} ({} days)", days + 1);
    let prices = load_prices(&region, start, today).await;
    if prices.len() < 48 {
        eprintln!("Not enough price data ({} points). Aborting.", prices.len());
        return;
    }

    let jobs = demo_jobs();

    println!();
    println!(
        "spotwatt backtest — region {region}, {} hourly price points over ~{} days",
        prices.len(),
        days + 1
    );
    println!("typical = run at no particular time · spotwatt = realistic 24–48h foresight · oracle = perfect foresight\n");

    // Header
    println!(
        "{:<17}{:>5}{:>7}{:>7}{:>12}{:>12}{:>9}{:>12}{:>9}",
        "job", "dur", "by", "power", "typical", "spotwatt", "saved", "oracle", "capture"
    );
    println!("{}", "─".repeat(90));

    let mut tot = JobResult { typical: 0.0, spotwatt: 0.0, oracle: 0.0, runs: 0 };
    for job in &jobs {
        let r = simulate_job(&prices, job, start, today);
        if r.runs == 0 {
            continue;
        }
        print_row(job, &r);
        tot.typical += r.typical;
        tot.spotwatt += r.spotwatt;
        tot.oracle += r.oracle;
        tot.runs += r.runs;
    }

    println!("{}", "─".repeat(90));
    let saved = pct(tot.typical, tot.spotwatt);
    let capture = capture_pct(tot.typical, tot.spotwatt, tot.oracle);
    println!(
        "{:<17}{:>5}{:>7}{:>7}{:>12}{:>12}{:>9}{:>12}{:>9}",
        "TOTAL",
        "",
        "",
        "",
        kr(tot.typical),
        kr(tot.spotwatt),
        format!("{saved:.0}%"),
        kr(tot.oracle),
        format!("{capture:.0}%"),
    );
    println!(
        "\nOver this period, cheapest-window scheduling would have cost {} instead of {} — saving {} ({:.0}%).",
        kr(tot.spotwatt),
        kr(tot.typical),
        kr(tot.typical - tot.spotwatt),
        saved
    );
    println!(
        "That captured {:.0}% of the {} that perfect foresight could theoretically save.",
        capture,
        kr(tot.typical - tot.oracle)
    );
}

/// Simulate one recurring job across every eligible day in the range.
fn simulate_job(full: &PriceSeries, job: &DemoJob, start: NaiveDate, end: NaiveDate) -> JobResult {
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

// --- helpers -------------------------------------------------------------

fn demo_jobs() -> Vec<DemoJob> {
    vec![
        DemoJob { name: "nightly-backup", duration_minutes: 45, deadline_hour: 7, power_watts: 60.0, window_hours: 24 },
        DemoJob { name: "photo-dedup", duration_minutes: 90, deadline_hour: 8, power_watts: 90.0, window_hours: 24 },
        DemoJob { name: "media-transcode", duration_minutes: 180, deadline_hour: 8, power_watts: 120.0, window_hours: 24 },
        DemoJob { name: "llm-finetune", duration_minutes: 360, deadline_hour: 9, power_watts: 250.0, window_hours: 24 },
    ]
}

fn print_row(job: &DemoJob, r: &JobResult) {
    let saved = pct(r.typical, r.spotwatt);
    let capture = capture_pct(r.typical, r.spotwatt, r.oracle);
    println!(
        "{:<17}{:>5}{:>7}{:>7}{:>12}{:>12}{:>9}{:>12}{:>9}",
        job.name,
        dur_label(job.duration_minutes),
        format!("{:02}:00", job.deadline_hour),
        format!("{:.0}W", job.power_watts),
        kr(r.typical),
        kr(r.spotwatt),
        format!("{saved:.0}%"),
        kr(r.oracle),
        format!("{capture:.0}%"),
    );
}

fn pct(base: f64, value: f64) -> f64 {
    if base > 0.0 {
        (base - value) / base * 100.0
    } else {
        0.0
    }
}

fn capture_pct(typical: f64, spotwatt: f64, oracle: f64) -> f64 {
    let achievable = typical - oracle;
    if achievable > 1e-9 {
        ((typical - spotwatt) / achievable * 100.0).clamp(0.0, 100.0)
    } else {
        100.0
    }
}

fn kr(v: f64) -> String {
    format!("{v:.2} kr")
}

fn dur_label(minutes: i64) -> String {
    if minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else if minutes > 60 {
        format!("{}h{}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

fn next<T: NextDay>(d: T) -> T {
    d.next_day()
}

/// Tiny trait so `next()` works for both dates and is explicit about intent.
trait NextDay {
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

fn region_from_str(s: &str) -> PriceRegion {
    match s.to_ascii_uppercase().as_str() {
        "NO2" => PriceRegion::NO2,
        "NO3" => PriceRegion::NO3,
        "NO4" => PriceRegion::NO4,
        "NO5" => PriceRegion::NO5,
        _ => PriceRegion::NO1,
    }
}

fn parse_args() -> (String, u64) {
    let mut region = "NO1".to_string();
    let mut days = 365u64;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--region" => {
                if let Some(v) = args.get(i + 1) {
                    region = v.clone();
                    i += 1;
                }
            }
            "--days" => {
                if let Some(v) = args.get(i + 1).and_then(|v| v.parse().ok()) {
                    days = v;
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    (region, days)
}

/// Load historical prices, caching fetched days on disk so reruns are instant
/// and we don't hammer the API.
async fn load_prices(region: &str, start: NaiveDate, end: NaiveDate) -> PriceSeries {
    let path = format!("backtest-cache-{region}.json");
    let mut cache: BTreeMap<String, Vec<PricePoint>> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let client = Strompris::default();
    let pr = region_from_str(region);
    let total = (end - start).num_days() + 1;

    let mut date = start;
    let mut fetched = 0u32;
    let mut idx = 0u32;
    while date <= end {
        idx += 1;
        let key = date.to_string();
        if !cache.contains_key(&key) {
            match client.get_prices(date, pr).await {
                Ok(hours) => {
                    let pts: Vec<PricePoint> = hours
                        .into_iter()
                        .map(|h| PricePoint {
                            start: h.time_start.with_timezone(&Utc),
                            end: h.time_end.with_timezone(&Utc),
                            nok_per_kwh: h.nok_per_kwh,
                            eur_per_kwh: h.eur_per_kwh,
                        })
                        .collect();
                    cache.insert(key, pts);
                    fetched += 1;
                    if fetched % 25 == 0 {
                        eprintln!("  fetched {fetched} new days ({idx}/{total})");
                    }
                    tokio::time::sleep(StdDuration::from_millis(80)).await;
                }
                Err(e) => {
                    // Missing day (e.g. before the API's 2021-12-01 floor).
                    eprintln!("  no data for {date}: {e:?}");
                    cache.insert(key, Vec::new());
                }
            }
        }
        date = next(date);
    }

    if let Ok(s) = serde_json::to_string(&cache) {
        let _ = std::fs::write(&path, s);
    }
    if fetched > 0 {
        eprintln!("  fetched {fetched} new days, rest from cache");
    } else {
        eprintln!("  all days served from cache");
    }

    let mut points = Vec::new();
    for (k, v) in &cache {
        if let Ok(d) = k.parse::<NaiveDate>() {
            if d >= start && d <= end {
                points.extend(v.iter().cloned());
            }
        }
    }
    PriceSeries::new(points)
}
