use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How a job decides *when* to run.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Policy {
    /// Run right away regardless of price (for things you can't defer).
    Immediate,
    /// Run as soon as the current hour's price drops to/below the threshold
    /// (NOK/kWh). A deadline, if set, can still force a run.
    Threshold { max_nok_per_kwh: f64 },
    /// Find the cheapest contiguous block of hours long enough to fit the job,
    /// optionally finishing before a deadline. This is the workhorse policy for
    /// transcoding, backups, model training, etc.
    CheapestWindow,
}

// With no policy specified (e.g. a JSON API request that omits the field),
// this is what spotwatt assumes you want.
#[allow(clippy::derivable_impls)]
impl Default for Policy {
    fn default() -> Self {
        Policy::CheapestWindow
    }
}

/// Used to order jobs when the concurrency cap forces a choice. Declared
/// lowest-to-highest so the derived `Ord` ranks `Critical` greatest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

/// The minimal description the scheduling algorithm needs. The full job record
/// (command, status, run history, …) lives in the server; this is just the part
/// that drives the *when* decision.
#[derive(Debug, Clone)]
pub struct JobSpec {
    pub policy: Policy,
    pub duration_minutes: i64,
    pub deadline: Option<DateTime<Utc>>,
    /// Do not start before this instant, regardless of policy or price.
    /// Recurring jobs set it on the next occurrence so a "daily" job can't
    /// fire again the same day it just ran.
    pub earliest_start: Option<DateTime<Utc>>,
}
