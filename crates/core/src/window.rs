//! Finding contiguous blocks of cheap hours in a price series — the search
//! half of scheduling. The decision half (run now or wait?) lives in
//! [`crate::schedule`].

use chrono::{DateTime, Duration, DurationRound, Utc};

use crate::price::PriceSeries;

/// A contiguous block of `hours` hourly slots and its cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub start_index: usize,
    pub hours: usize,
    /// Mean NOK/kWh across the window.
    pub avg_nok: f64,
    /// Sum of NOK/kWh across the window (proportional to energy cost at
    /// constant power, so it ranks windows correctly).
    pub sum_nok: f64,
}

/// Floor an instant to the start of its hour.
pub fn hour_floor(t: DateTime<Utc>) -> DateTime<Utc> {
    t.duration_trunc(Duration::hours(1)).unwrap_or(t)
}

/// Whole hourly slots a job of `minutes` occupies (at least one).
pub fn slots_for(minutes: i64) -> usize {
    let m = minutes.max(0) as f64;
    (m / 60.0).ceil().max(1.0) as usize
}

/// Cheapest contiguous window of `k` hourly slots whose start is at or after
/// `earliest` and (if `deadline` is set) whose end is at or before it. Ties are
/// broken toward the earliest start.
pub fn cheapest_window(
    prices: &PriceSeries,
    earliest: DateTime<Utc>,
    deadline: Option<DateTime<Utc>>,
    k: usize,
) -> Option<Window> {
    let pts = &prices.points;
    if k == 0 || pts.len() < k {
        return None;
    }

    let mut best: Option<Window> = None;
    for i in 0..=pts.len() - k {
        // Only consider genuinely contiguous slots (no gaps in the data).
        let mut contiguous = true;
        for j in i..i + k - 1 {
            if pts[j].end != pts[j + 1].start {
                contiguous = false;
                break;
            }
        }
        if !contiguous {
            continue;
        }

        let start = pts[i].start;
        let end = pts[i + k - 1].end;
        if start < earliest {
            continue;
        }
        if let Some(dl) = deadline {
            if end > dl {
                continue;
            }
        }

        let sum: f64 = pts[i..i + k].iter().map(|p| p.nok_per_kwh).sum();
        // Strict `<` keeps the earliest window on ties, since `i` ascends.
        let better = best.as_ref().map(|b| sum < b.sum_nok).unwrap_or(true);
        if better {
            best = Some(Window {
                start,
                end,
                start_index: i,
                hours: k,
                avg_nok: sum / k as f64,
                sum_nok: sum,
            });
        }
    }
    best
}

/// The window of `k` slots beginning at the first slot not already in the past
/// relative to `from`. Used to price the "if it ran now" baseline.
pub fn window_at(prices: &PriceSeries, from: DateTime<Utc>, k: usize) -> Option<Window> {
    let pts = &prices.points;
    if k == 0 {
        return None;
    }
    let i = pts.iter().position(|p| p.end > from)?;
    if i + k > pts.len() {
        return None;
    }
    for j in i..i + k - 1 {
        if pts[j].end != pts[j + 1].start {
            return None;
        }
    }
    let sum: f64 = pts[i..i + k].iter().map(|p| p.nok_per_kwh).sum();
    Some(Window {
        start: pts[i].start,
        end: pts[i + k - 1].end,
        start_index: i,
        hours: k,
        avg_nok: sum / k as f64,
        sum_nok: sum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{base, series};
    use chrono::Duration;

    #[test]
    fn cheapest_window_picks_min_average() {
        // cheapest 2h block is indices 2-3 (0.2 + 0.3).
        let s = series(base(), &[1.0, 0.5, 0.2, 0.3, 0.9]);
        let w = cheapest_window(&s, base(), None, 2).unwrap();
        assert_eq!(w.start_index, 2);
    }

    #[test]
    fn cheapest_window_respects_deadline() {
        // Cheapest block is at the end, but the deadline excludes it.
        let s = series(base(), &[1.0, 0.9, 0.8, 0.1, 0.1]);
        let dl = base() + Duration::hours(3); // windows must end by hour 3
        let w = cheapest_window(&s, base(), Some(dl), 2).unwrap();
        assert_eq!(w.start_index, 1); // 0.9 + 0.8 beats 1.0 + 0.9
    }

    #[test]
    fn cheapest_window_breaks_ties_toward_earliest() {
        let s = series(base(), &[0.2, 0.2, 0.2, 0.2]);
        let w = cheapest_window(&s, base(), None, 2).unwrap();
        assert_eq!(w.start_index, 0);
    }
}
