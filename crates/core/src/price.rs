use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
