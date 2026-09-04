//! Wire types and an HTTP client for the spotwatt JSON API
//! (`crates/server/src/web/api.rs`). Deliberately independent of the server
//! crate — this is what an external client sees, over the wire, nothing more.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How a job decides *when* to run. JSON-tagged as `{"kind": "...", ...}`,
/// matching the server's `spotwatt_core::Policy`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Policy {
    Immediate,
    Threshold { max_nok_per_kwh: f64 },
    CheapestWindow,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    None,
    Daily,
}

/// Body for `POST /api/jobs`.
#[derive(Debug, Serialize)]
pub struct CreateJobRequest {
    pub name: String,
    pub command: String,
    pub policy: Policy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_watts: Option<f64>,
    pub priority: Priority,
    pub repeat: Repeat,
}

/// A job record as the server returns it. Only the fields the CLI displays
/// are modeled; anything else in the response is ignored by serde.
#[derive(Debug, Deserialize)]
pub struct Job {
    pub id: i64,
    pub name: String,
    pub command: String,
    pub status: String,
    pub duration_minutes: i64,
    pub deadline: Option<DateTime<Utc>>,
    pub power_watts: Option<f64>,
    pub priority: String,
    pub repeat: String,
    pub scheduled_start: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i64>,
    pub output: Option<String>,
    pub est_cost_nok: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct PricePoint {
    pub start: DateTime<Utc>,
    #[allow(dead_code)]
    pub end: DateTime<Utc>,
    pub nok_per_kwh: f64,
}

#[derive(Debug, Deserialize)]
pub struct PriceSeries {
    pub points: Vec<PricePoint>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    errors: Vec<String>,
}

pub struct Client {
    base_url: String,
    http: reqwest::blocking::Client,
}

impl Client {
    pub fn new(base_url: String) -> Self {
        Client {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: reqwest::blocking::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Turn a non-2xx response into a readable error, preferring the API's
    /// own `{"errors": [...]}` body when present.
    fn check(resp: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().unwrap_or_default();
        if let Ok(err) = serde_json::from_str::<ApiError>(&body) {
            bail!("{status}: {}", err.errors.join("; "));
        }
        bail!("{status}: {body}");
    }

    pub fn list_jobs(&self) -> Result<Vec<Job>> {
        let resp = self
            .http
            .get(self.url("/api/jobs"))
            .send()
            .context("connecting to spotwatt server")?;
        Self::check(resp)?.json().context("parsing job list")
    }

    pub fn get_job(&self, id: i64) -> Result<Job> {
        let resp = self
            .http
            .get(self.url(&format!("/api/jobs/{id}")))
            .send()
            .context("connecting to spotwatt server")?;
        Self::check(resp)?.json().context("parsing job")
    }

    pub fn create_job(&self, req: &CreateJobRequest) -> Result<Job> {
        let resp = self
            .http
            .post(self.url("/api/jobs"))
            .json(req)
            .send()
            .context("connecting to spotwatt server")?;
        Self::check(resp)?.json().context("parsing created job")
    }

    pub fn cancel_job(&self, id: i64) -> Result<Job> {
        let resp = self
            .http
            .post(self.url(&format!("/api/jobs/{id}/cancel")))
            .send()
            .context("connecting to spotwatt server")?;
        Self::check(resp)?.json().context("parsing job")
    }

    pub fn run_job_now(&self, id: i64) -> Result<Job> {
        let resp = self
            .http
            .post(self.url(&format!("/api/jobs/{id}/run")))
            .send()
            .context("connecting to spotwatt server")?;
        Self::check(resp)?.json().context("parsing job")
    }

    pub fn delete_job(&self, id: i64) -> Result<()> {
        let resp = self
            .http
            .delete(self.url(&format!("/api/jobs/{id}")))
            .send()
            .context("connecting to spotwatt server")?;
        Self::check(resp)?;
        Ok(())
    }

    pub fn prices(&self) -> Result<PriceSeries> {
        let resp = self
            .http
            .get(self.url("/api/prices"))
            .send()
            .context("connecting to spotwatt server")?;
        Self::check(resp)?.json().context("parsing prices")
    }
}

/// A connection error is the single most common first-run failure (server
/// not started, wrong `--url`) — give it a pointed message instead of a raw
/// reqwest error.
pub fn friendly_connect_error(e: &anyhow::Error, base_url: &str) -> anyhow::Error {
    if e.chain()
        .any(|c| c.to_string().contains("connecting to spotwatt server"))
    {
        return anyhow!(
            "couldn't reach a spotwatt server at {base_url} — is it running? \
             (start it with `cargo run -p spotwatt-server`, or point --url / SPOTWATT_URL at it)"
        );
    }
    anyhow!("{e}")
}
