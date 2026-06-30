use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use chrono_tz::Europe::Oslo;
use spotwatt_core::{PricePoint, PriceSeries};
use strompris::{PriceRegion, Strompris};

use crate::AppState;

fn region_from_str(s: &str) -> PriceRegion {
    match s.to_ascii_uppercase().as_str() {
        "NO2" => PriceRegion::NO2,
        "NO3" => PriceRegion::NO3,
        "NO4" => PriceRegion::NO4,
        "NO5" => PriceRegion::NO5,
        _ => PriceRegion::NO1,
    }
}

/// Fetch today's and (if published) tomorrow's prices for `region` and merge
/// them into a single UTC series. Tomorrow 404s before ~13:00 local; that's
/// expected and not an error.
pub async fn fetch_series(region: &str) -> Result<PriceSeries> {
    let client = Strompris::default();
    let pr = region_from_str(region);

    // The API is keyed by the Oslo-local calendar date.
    let today = Utc::now().with_timezone(&Oslo).date_naive();
    let tomorrow = today + chrono::Duration::days(1);

    let mut points: Vec<PricePoint> = Vec::new();
    for date in [today, tomorrow] {
        match client.get_prices(date, pr).await {
            Ok(hours) => {
                for h in hours {
                    points.push(PricePoint {
                        start: h.time_start.with_timezone(&Utc),
                        end: h.time_end.with_timezone(&Utc),
                        nok_per_kwh: h.nok_per_kwh,
                        eur_per_kwh: h.eur_per_kwh,
                    });
                }
            }
            Err(e) => {
                // Most often: tomorrow not published yet.
                tracing::debug!("no prices for {date}: {e:?}");
            }
        }
    }

    Ok(PriceSeries::new(points))
}

/// Refresh prices on startup and then on the configured interval, forever.
pub async fn refresh_loop(state: Arc<AppState>) {
    let interval = Duration::from_secs(state.config.price_refresh_minutes.max(1) * 60);
    loop {
        match fetch_series(&state.config.region).await {
            Ok(series) => {
                let n = series.len();
                *state.prices.write().await = series;
                tracing::info!("loaded {n} price points for {}", state.config.region);
            }
            Err(e) => tracing::warn!("price fetch failed: {e:?}"),
        }
        tokio::time::sleep(interval).await;
    }
}
