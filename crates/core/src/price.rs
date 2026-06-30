use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::tariff::Tariff;

/// A single hourly spot-price point. Times are stored in UTC internally;
/// conversion to Europe/Oslo happens only at the display edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricePoint {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub nok_per_kwh: f64,
    pub eur_per_kwh: f64,
}

impl PricePoint {
    /// Whether instant `t` falls inside this hour (`[start, end)`).
    pub fn contains(&self, t: DateTime<Utc>) -> bool {
        t >= self.start && t < self.end
    }
}

/// An ordered series of hourly price points, sorted ascending by start time
/// with duplicate starts removed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PriceSeries {
    pub points: Vec<PricePoint>,
}

impl PriceSeries {
    pub fn new(mut points: Vec<PricePoint>) -> Self {
        points.sort_by(|a, b| a.start.cmp(&b.start));
        // `dedup_by` removes the *later* of two equal elements, so the earliest
        // fetched value for a given hour wins.
        points.dedup_by(|a, b| a.start == b.start);
        Self { points }
    }

    /// A copy of this series with every hour's `nok_per_kwh` replaced by the
    /// effective consumer price under `tariff`. This is the series scheduling
    /// and cost estimation should run against — it is what the customer actually
    /// pays, and (because strømstøtte flattens the top of the curve) it ranks
    /// hours differently from raw spot. The raw `eur_per_kwh` is left untouched.
    pub fn with_tariff(&self, tariff: &Tariff) -> PriceSeries {
        let points = self
            .points
            .iter()
            .map(|p| PricePoint {
                start: p.start,
                end: p.end,
                nok_per_kwh: tariff.effective(p.nok_per_kwh),
                eur_per_kwh: p.eur_per_kwh,
            })
            .collect();
        // Already sorted and deduped; reuse the constructor for invariants.
        PriceSeries::new(points)
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Price point covering instant `t`, if known.
    pub fn price_at(&self, t: DateTime<Utc>) -> Option<&PricePoint> {
        self.points.iter().find(|p| p.contains(t))
    }

    pub fn first_start(&self) -> Option<DateTime<Utc>> {
        self.points.first().map(|p| p.start)
    }

    /// End of the known horizon (exclusive), i.e. the last hour's end time.
    pub fn last_end(&self) -> Option<DateTime<Utc>> {
        self.points.last().map(|p| p.end)
    }

    pub fn min_point(&self) -> Option<&PricePoint> {
        self.points
            .iter()
            .min_by(|a, b| a.nok_per_kwh.total_cmp(&b.nok_per_kwh))
    }

    pub fn max_point(&self) -> Option<&PricePoint> {
        self.points
            .iter()
            .max_by(|a, b| a.nok_per_kwh.total_cmp(&b.nok_per_kwh))
    }

    pub fn avg_nok(&self) -> Option<f64> {
        if self.points.is_empty() {
            None
        } else {
            let sum: f64 = self.points.iter().map(|p| p.nok_per_kwh).sum();
            Some(sum / self.points.len() as f64)
        }
    }
}
