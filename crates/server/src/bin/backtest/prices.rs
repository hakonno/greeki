//! Historical price loading, with an on-disk per-day cache so reruns are
//! instant and we don't hammer the API.

use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

use chrono::{NaiveDate, Utc};
use spotwatt_core::{PricePoint, PriceSeries};
use strompris::{PriceRegion, Strompris};

use crate::sim::next;

/// Load historical prices, caching fetched days on disk so reruns are instant
/// and we don't hammer the API.
pub async fn load_prices(region: &str, start: NaiveDate, end: NaiveDate) -> PriceSeries {
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

fn region_from_str(s: &str) -> PriceRegion {
    match s.to_ascii_uppercase().as_str() {
        "NO2" => PriceRegion::NO2,
        "NO3" => PriceRegion::NO3,
        "NO4" => PriceRegion::NO4,
        "NO5" => PriceRegion::NO5,
        _ => PriceRegion::NO1,
    }
}
