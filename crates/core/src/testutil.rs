//! Shared helpers for the crate's unit tests.

use chrono::{DateTime, Duration, TimeZone, Utc};

use crate::price::{PricePoint, PriceSeries};

/// Build an hourly series starting at `start` with the given NOK/kWh prices.
pub fn series(start: DateTime<Utc>, prices: &[f64]) -> PriceSeries {
    let pts = prices
        .iter()
        .enumerate()
        .map(|(i, &pr)| {
            let s = start + Duration::hours(i as i64);
            PricePoint {
                start: s,
                end: s + Duration::hours(1),
                nok_per_kwh: pr,
                eur_per_kwh: pr / 11.0,
            }
        })
        .collect();
    PriceSeries::new(pts)
}

/// A fixed, boring reference instant for tests.
pub fn base() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 30, 0, 0, 0).unwrap()
}
