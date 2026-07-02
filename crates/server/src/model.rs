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
    pub created_at: DateTime<Utc>,
}
