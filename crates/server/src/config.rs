use serde::Deserialize;

/// Runtime configuration, loaded from a TOML file (path in `SPOTWATT_CONFIG`,
/// default `config.toml`) with a couple of env-var overrides for convenience.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Price region: NO1..NO5.
    #[serde(default = "d_region")]
    pub region: String,
    /// Address the HTTP dashboard listens on.
    #[serde(default = "d_listen")]
    pub listen: String,
    /// sqlx connection string.
    #[serde(default = "d_db")]
    pub database_url: String,
    /// How often the scheduler re-evaluates jobs.
    #[serde(default = "d_tick")]
    pub tick_seconds: u64,
    /// Maximum jobs allowed to run at the same time.
    #[serde(default = "d_max_jobs")]
    pub max_concurrent_jobs: usize,
    /// How often to re-fetch the price curve.
    #[serde(default = "d_refresh")]
    pub price_refresh_minutes: u64,
}

fn d_region() -> String {
    "NO1".to_string()
}
fn d_listen() -> String {
    "127.0.0.1:8080".to_string()
}
fn d_db() -> String {
    "sqlite:spotwatt.db?mode=rwc".to_string()
}
fn d_tick() -> u64 {
    60
}
fn d_max_jobs() -> usize {
    2
}
fn d_refresh() -> u64 {
    30
}

impl Default for Config {
    fn default() -> Self {
        Config {
            region: d_region(),
            listen: d_listen(),
            database_url: d_db(),
            tick_seconds: d_tick(),
            max_concurrent_jobs: d_max_jobs(),
            price_refresh_minutes: d_refresh(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = std::env::var("SPOTWATT_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str::<Config>(&s).unwrap_or_else(|e| {
                eprintln!("warning: failed to parse {path}: {e}; using defaults");
                Config::default()
            }),
            Err(_) => Config::default(),
        };
        if let Ok(r) = std::env::var("SPOTWATT_REGION") {
            cfg.region = r;
        }
        if let Ok(l) = std::env::var("SPOTWATT_LISTEN") {
            cfg.listen = l;
        }
        cfg
    }
}
