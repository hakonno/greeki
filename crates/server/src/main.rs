mod config;
mod db;
mod executor;
mod model;
mod prices;
mod scheduler;
mod web;

use std::sync::Arc;

use spotwatt_core::PriceSeries;
use tokio::sync::{Notify, RwLock};
use tracing_subscriber::EnvFilter;

/// Everything the request handlers and background tasks share.
pub struct AppState {
    pub db: sqlx::SqlitePool,
    /// Latest known price curve. Refreshed by the price task, read everywhere.
    pub prices: RwLock<PriceSeries>,
    pub config: config::Config,
    /// Poked whenever something happens that could make a job startable right
    /// now — a job is created, or a running one finishes and frees a slot — so
    /// the scheduler re-plans immediately instead of waiting out its tick.
    pub kick: Notify,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = config::Config::load();
    tracing::info!(region = %config.region, "starting spotwatt");

    let db = db::init(&config.database_url).await?;

    // Any job left "running" is an orphan from a previous process that died
    // mid-run; clear it so its concurrency slot is freed and state is honest.
    match db::reconcile_orphans(&db, chrono::Utc::now().timestamp()).await {
        Ok(0) => {}
        Ok(n) => tracing::warn!("reconciled {n} orphaned running job(s) from a previous run"),
        Err(e) => tracing::warn!("orphan reconciliation failed: {e:?}"),
    }

    let state = Arc::new(AppState {
        db,
        prices: RwLock::new(PriceSeries::default()),
        config: config.clone(),
        kick: Notify::new(),
    });

    // Background workers: keep prices fresh and re-plan jobs on every tick.
    tokio::spawn(prices::refresh_loop(state.clone()));
    tokio::spawn(scheduler::run(state.clone()));

    let app = web::router(state.clone());
    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    tracing::info!("dashboard on http://{}", config.listen);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
