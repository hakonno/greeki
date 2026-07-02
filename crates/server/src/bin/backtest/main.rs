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

mod prices;
mod sim;

use chrono::{Days, Utc};
use chrono_tz::Europe::Oslo;

use sim::{simulate_job, DemoJob, JobResult};

#[tokio::main]
async fn main() {
    let (region, days) = parse_args();

    let today = Utc::now().with_timezone(&Oslo).date_naive();
    let start = today - Days::new(days);

    eprintln!("Loading prices for {region}: {start} … {today} ({} days)", days + 1);
    let prices = prices::load_prices(&region, start, today).await;
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
