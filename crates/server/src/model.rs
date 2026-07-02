use chrono::{DateTime, Utc};
use serde::Serialize;
use spotwatt_core::{Policy, Priority};

/// Lifecycle state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Running => "running",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "running" => JobStatus::Running,
            "completed" => JobStatus::Completed,
            "failed" => JobStatus::Failed,
            "cancelled" => JobStatus::Cancelled,
            _ => JobStatus::Pending,
        }
    }
}

/// How (and whether) a job re-creates itself after it finishes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    /// Single-shot: runs once and is done.
    None,
    /// On completion, schedule the same job again for the next day (any deadline
    /// rolls forward 24h). Makes "nightly backup before 07:00" actually nightly.
    Daily,
}

impl Repeat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Repeat::None => "none",
            Repeat::Daily => "daily",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "daily" => Repeat::Daily,
            _ => Repeat::None,
        }
    }
}

/// A full job record as persisted and shown in the dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: i64,
    pub name: String,
    pub command: String,
    pub policy: Policy,
    pub duration_minutes: i64,
    pub deadline: Option<DateTime<Utc>>,
    pub power_watts: Option<f64>,
    pub priority: Priority,
    pub repeat: Repeat,
    /// Earliest instant the job may start; recurring jobs use it to hold the
    /// next occurrence back until its own day.
    pub earliest_start: Option<DateTime<Utc>>,
    pub status: JobStatus,
    pub scheduled_start: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i64>,
    pub output: Option<String>,
    pub est_cost_nok: Option<f64>,
}

impl Job {
    /// Estimated average power draw, in kW.
    pub fn power_kw(&self) -> Option<f64> {
        self.power_watts.map(|w| w / 1000.0)
    }
}

/// Fields needed to create a new job.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub name: String,
    pub command: String,
    pub policy: Policy,
    pub duration_minutes: i64,
    pub deadline: Option<DateTime<Utc>>,
    pub power_watts: Option<f64>,
    pub priority: Priority,
    pub repeat: Repeat,
    pub earliest_start: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
