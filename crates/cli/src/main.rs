//! `spotwatt-cli` — a command-line client for a running spotwatt server.
//!
//! It talks to the server's JSON API (`/api/...`) over plain HTTP; it does
//! not run any scheduling logic itself; the server it's pointed at is
//! whoever actually decides when a job runs. See the top-level README for
//! what spotwatt is. Like the rest of this project, it assumes a Norwegian
//! deployment (NO1–NO5 spot prices, Norwegian grid tariffs) — nothing here
//! is region-specific by itself, but the server behind it is.

mod api;
mod deadline;

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use api::{friendly_connect_error, Client, CreateJobRequest, Job, Policy, Priority, Repeat};

/// Command-line client for a running spotwatt server.
#[derive(Parser)]
#[command(name = "spotwatt-cli", version, about, long_about = None)]
struct Cli {
    /// Base URL of the spotwatt server.
    #[arg(
        long,
        env = "SPOTWATT_URL",
        default_value = "http://127.0.0.1:8080",
        global = true
    )]
    url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Submit a new job.
    Add(AddArgs),
    /// List all jobs.
    #[command(alias = "ls")]
    List,
    /// Show one job's full detail, including captured output.
    Show { id: i64 },
    /// Cancel a pending job.
    Cancel { id: i64 },
    /// Delete a job record.
    #[command(alias = "delete")]
    Rm { id: i64 },
    /// Start a pending job immediately, ignoring price.
    RunNow { id: i64 },
    /// Show the current power-price curve (raw spot, kr/kWh).
    Price,
}

#[derive(clap::Args)]
struct AddArgs {
    /// Human-readable name for the job.
    #[arg(long)]
    name: String,

    /// When to run it.
    #[arg(long, value_enum, default_value_t = PolicyArg::CheapestWindow)]
    policy: PolicyArg,

    /// Required when --policy=threshold: run once the price drops to/below
    /// this many kr/kWh.
    #[arg(long)]
    threshold: Option<f64>,

    /// Shorthand for --policy=immediate: run right now regardless of price.
    #[arg(long, conflicts_with = "policy")]
    now: bool,

    /// Expected run time in whole minutes (default 60).
    #[arg(long)]
    duration: Option<i64>,

    /// Must finish by this time: `HH:MM` (next occurrence, Europe/Oslo) or a
    /// full RFC 3339 timestamp.
    #[arg(long)]
    deadline: Option<String>,

    /// Expected power draw in watts, for the site power budget.
    #[arg(long)]
    power: Option<f64>,

    /// Tie-breaker when the concurrency cap forces a choice.
    #[arg(long, value_enum, default_value_t = PriorityArg::Normal)]
    priority: PriorityArg,

    /// Re-queue for the next day on completion.
    #[arg(long)]
    daily: bool,

    /// The command to run, e.g. `-- rsync -a /data backup:/`.
    #[arg(last = true, required = true)]
    cmd: Vec<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum PolicyArg {
    CheapestWindow,
    Threshold,
    Immediate,
}

#[derive(Clone, Copy, ValueEnum)]
enum PriorityArg {
    Low,
    Normal,
    High,
    Critical,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    let client = Client::new(cli.url.clone());
    match &cli.command {
        Command::Add(args) => add(&client, args).map_err(|e| friendly_connect_error(&e, &cli.url)),
        Command::List => list(&client).map_err(|e| friendly_connect_error(&e, &cli.url)),
        Command::Show { id } => {
            show(&client, *id).map_err(|e| friendly_connect_error(&e, &cli.url))
        }
        Command::Cancel { id } => {
            let job = client
                .cancel_job(*id)
                .map_err(|e| friendly_connect_error(&e, &cli.url))?;
            println!("cancelled #{} \"{}\" ({})", job.id, job.name, job.status);
            Ok(())
        }
        Command::Rm { id } => {
            client
                .delete_job(*id)
                .map_err(|e| friendly_connect_error(&e, &cli.url))?;
            println!("deleted #{id}");
            Ok(())
        }
        Command::RunNow { id } => {
            let job = client
                .run_job_now(*id)
                .map_err(|e| friendly_connect_error(&e, &cli.url))?;
            println!("started #{} \"{}\" ({})", job.id, job.name, job.status);
            Ok(())
        }
        Command::Price => price(&client).map_err(|e| friendly_connect_error(&e, &cli.url)),
    }
}

fn add(client: &Client, args: &AddArgs) -> Result<()> {
    let policy = if args.now {
        Policy::Immediate
    } else {
        match args.policy {
            PolicyArg::Immediate => Policy::Immediate,
            PolicyArg::CheapestWindow => Policy::CheapestWindow,
            PolicyArg::Threshold => {
                let max = args
                    .threshold
                    .context("--policy=threshold needs --threshold <kr/kWh>")?;
                Policy::Threshold {
                    max_nok_per_kwh: max,
                }
            }
        }
    };

    let deadline = args.deadline.as_deref().map(deadline::parse).transpose()?;

    let req = CreateJobRequest {
        name: args.name.clone(),
        command: args.cmd.join(" "),
        policy,
        duration_minutes: args.duration,
        deadline,
        power_watts: args.power,
        priority: match args.priority {
            PriorityArg::Low => Priority::Low,
            PriorityArg::Normal => Priority::Normal,
            PriorityArg::High => Priority::High,
            PriorityArg::Critical => Priority::Critical,
        },
        repeat: if args.daily {
            Repeat::Daily
        } else {
            Repeat::None
        },
    };

    let job = client.create_job(&req)?;
    println!("created #{} \"{}\" — {}", job.id, job.name, job.status);
    Ok(())
}

fn list(client: &Client) -> Result<()> {
    let jobs = client.list_jobs()?;
    if jobs.is_empty() {
        println!("no jobs yet — see `spotwatt-cli add --help`");
        return Ok(());
    }
    println!(
        "{:<5} {:<10} {:<24} {:<20} COST",
        "ID", "STATUS", "NAME", "SCHEDULED / DEADLINE"
    );
    for job in jobs {
        let when = job
            .scheduled_start
            .map(|t| format!("start {}", t.format("%m-%d %H:%M")))
            .or_else(|| {
                job.deadline
                    .map(|t| format!("by {}", t.format("%m-%d %H:%M")))
            })
            .unwrap_or_else(|| "-".to_string());
        let cost = job
            .est_cost_nok
            .map(|c| format!("{c:.2} kr"))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<5} {:<10} {:<24} {:<20} {}",
            job.id,
            job.status,
            truncate(&job.name, 24),
            when,
            cost
        );
    }
    Ok(())
}

fn show(client: &Client, id: i64) -> Result<()> {
    let job: Job = client.get_job(id)?;
    println!("#{} {}", job.id, job.name);
    println!("  command:   {}", job.command);
    println!("  status:    {}", job.status);
    println!("  priority:  {}", job.priority);
    println!("  repeat:    {}", job.repeat);
    println!("  duration:  {} min", job.duration_minutes);
    if let Some(w) = job.power_watts {
        println!("  power:     {w} W");
    }
    if let Some(d) = job.deadline {
        println!("  deadline:  {}", d.to_rfc3339());
    }
    if let Some(s) = job.scheduled_start {
        println!("  scheduled: {}", s.to_rfc3339());
    }
    if let Some(s) = job.started_at {
        println!("  started:   {}", s.to_rfc3339());
    }
    if let Some(f) = job.finished_at {
        println!("  finished:  {}", f.to_rfc3339());
    }
    if let Some(c) = job.exit_code {
        println!("  exit code: {c}");
    }
    if let Some(c) = job.est_cost_nok {
        println!("  cost:      {c:.2} kr");
    }
    if let Some(out) = job.output.filter(|o| !o.is_empty()) {
        println!("  output:\n{}", indent(&out, "    "));
    }
    Ok(())
}

fn price(client: &Client) -> Result<()> {
    let series = client.prices()?;
    if series.points.is_empty() {
        println!("no price data yet — the server hasn't fetched a curve");
        return Ok(());
    }
    println!("{:<20} SPOT kr/kWh (ex-VAT)", "HOUR (local)");
    for p in &series.points {
        let local = p.start.with_timezone(&chrono_tz::Europe::Oslo);
        println!(
            "{:<20} {:.3}",
            local.format("%a %m-%d %H:%M"),
            p.nok_per_kwh
        );
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|l| format!("{prefix}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
