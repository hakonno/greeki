use chrono::{DateTime, Utc};

use crate::price::PriceSeries;

/// Energy cost in NOK of drawing `power_kw` continuously over `[start, end)`,
/// integrated across the hourly price curve (handles partial-hour overlaps).
/// Returns `None` if no known price covers any part of the interval.
pub fn interval_cost(
    prices: &PriceSeries,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    power_kw: f64,
) -> Option<f64> {
    if end <= start {
        return Some(0.0);
    }
    let mut total = 0.0;
    let mut any = false;
    for p in &prices.points {
        let s = start.max(p.start);
        let e = end.min(p.end);
        if e > s {
            let hours = (e - s).num_seconds() as f64 / 3600.0;
            total += p.nok_per_kwh * power_kw * hours;
            any = true;
        }
    }
    if any {
        Some(total)
    } else {
        None
    }
}
