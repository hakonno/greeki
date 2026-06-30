//! The scheduling decision: given a job spec, a price series and the current
//! time, decide whether the job should start *now* and, if not, when the
//! cheapest opportunity is.
//!
//! Design note — we never commit to a far-future plan. The price API only knows
//! ~24–48h ahead, so every scheduler tick re-runs `plan` against the latest
//! known prices. A job "waits" simply by not being told to run yet; once a
//! cheaper hour becomes the current hour (or new prices reveal a better
//! window), the next tick picks it up. This keeps the logic stateless and
//! robust to the limited price horizon.

use chrono::{DateTime, Duration, DurationRound, Utc};

use crate::job::{JobSpec, Policy};
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

/// The outcome of a scheduling decision for one job.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Whether the job should start at this very tick.
    pub run_now: bool,
    /// When the job is expected to start (for display / persistence).
    pub start_at: Option<DateTime<Utc>>,
    /// True when a deadline forced a start that isn't price-optimal.
    pub forced: bool,
    /// Average price of the chosen window, when known.
    pub window_avg_nok: Option<f64>,
    /// Human-readable explanation, shown in the dashboard.
    pub reason: String,
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

/// Decide whether and when `spec` should run, given `prices` and the current
/// time `now`.
pub fn plan(spec: &JobSpec, prices: &PriceSeries, now: DateTime<Utc>) -> Plan {
    let duration = Duration::minutes(spec.duration_minutes.max(0));
    match spec.policy {
        Policy::Immediate => Plan {
            run_now: true,
            start_at: Some(now),
            forced: false,
            window_avg_nok: prices.price_at(now).map(|p| p.nok_per_kwh),
            reason: "immediate – runs regardless of price".to_string(),
        },
        Policy::Threshold { max_nok_per_kwh } => {
            plan_threshold(spec, prices, now, max_nok_per_kwh, duration)
        }
        Policy::CheapestWindow => plan_cheapest(spec, prices, now, duration),
    }
}

/// True when so little time is left before the deadline that the job must start
/// now to have any chance of finishing.
fn deadline_forces(deadline: Option<DateTime<Utc>>, now: DateTime<Utc>, duration: Duration) -> bool {
    matches!(deadline, Some(dl) if dl - now <= duration)
}

fn plan_threshold(
    spec: &JobSpec,
    prices: &PriceSeries,
    now: DateTime<Utc>,
    max: f64,
    duration: Duration,
) -> Plan {
    if deadline_forces(spec.deadline, now, duration) {
        return Plan {
            run_now: true,
            start_at: Some(now),
            forced: true,
            window_avg_nok: prices.price_at(now).map(|p| p.nok_per_kwh),
            reason: "deadline reached – starting despite price".to_string(),
        };
    }

    match prices.price_at(now) {
        Some(p) if p.nok_per_kwh <= max => Plan {
            run_now: true,
            start_at: Some(now),
            forced: false,
            window_avg_nok: Some(p.nok_per_kwh),
            reason: format!("price {:.3} kr ≤ threshold {:.3} kr", p.nok_per_kwh, max),
        },
        Some(p) => {
            let next = prices
                .points
                .iter()
                .find(|q| {
                    q.start > now
                        && q.nok_per_kwh <= max
                        && spec.deadline.map_or(true, |dl| q.end <= dl)
                })
                .map(|q| q.start);
            Plan {
                run_now: false,
                start_at: next,
                forced: false,
                window_avg_nok: Some(p.nok_per_kwh),
                reason: format!("price {:.3} kr > threshold {:.3} kr – waiting", p.nok_per_kwh, max),
            }
        }
        None => Plan {
            run_now: false,
            start_at: None,
            forced: false,
            window_avg_nok: None,
            reason: "no price data for the current hour".to_string(),
        },
    }
}

fn plan_cheapest(
    spec: &JobSpec,
    prices: &PriceSeries,
    now: DateTime<Utc>,
    duration: Duration,
) -> Plan {
    let k = slots_for(spec.duration_minutes);
    let earliest = hour_floor(now);
    let forced = deadline_forces(spec.deadline, now, duration);

    match cheapest_window(prices, earliest, spec.deadline, k) {
        Some(w) => {
            // The window can only start in the current hour (== now's hour) or
            // later, since `earliest` is the floor of `now`.
            let starts_now = w.start <= now;
            let run_now = starts_now || forced;
            let start_at = if forced && !starts_now {
                Some(now)
            } else {
                Some(w.start)
            };
            let reason = if starts_now {
                format!("cheapest {}-h window starts now (avg {:.3} kr)", k, w.avg_nok)
            } else if forced {
                "deadline reached – starting now (best effort)".to_string()
            } else {
                format!(
                    "cheapest {}-h window is later (avg {:.3} kr) – waiting",
                    k, w.avg_nok
                )
            };
            Plan {
                run_now,
                start_at,
                forced,
                window_avg_nok: Some(w.avg_nok),
                reason,
            }
        }
        None => {
            if forced {
                Plan {
                    run_now: true,
                    start_at: Some(now),
                    forced: true,
                    window_avg_nok: prices.price_at(now).map(|p| p.nok_per_kwh),
                    reason: "deadline reached – starting now (no full window fits)".to_string(),
                }
            } else {
                Plan {
                    run_now: false,
                    start_at: None,
                    forced: false,
                    window_avg_nok: None,
                    reason: "waiting for price data to cover a full window before the deadline"
                        .to_string(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price::{PricePoint, PriceSeries};
    use chrono::{TimeZone, Utc};

    /// Build an hourly series starting at `start` with the given NOK/kWh prices.
    fn series(start: DateTime<Utc>, prices: &[f64]) -> PriceSeries {
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

    fn base() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 30, 0, 0, 0).unwrap()
    }

    fn spec(policy: Policy, minutes: i64, deadline: Option<DateTime<Utc>>) -> JobSpec {
        JobSpec {
            policy,
            duration_minutes: minutes,
            deadline,
        }
    }

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

    #[test]
    fn plan_waits_when_cheapest_is_later() {
        let s = series(base(), &[1.0, 0.2, 0.2, 1.0]);
        let p = plan(&spec(Policy::CheapestWindow, 60, None), &s, base());
        assert!(!p.run_now);
        assert_eq!(p.start_at, Some(base() + Duration::hours(1)));
    }

    #[test]
    fn plan_runs_now_when_now_is_cheapest() {
        let s = series(base(), &[0.1, 0.9, 0.9]);
        let p = plan(&spec(Policy::CheapestWindow, 60, None), &s, base());
        assert!(p.run_now);
    }

    #[test]
    fn plan_forced_when_deadline_too_soon() {
        let s = series(base(), &[1.0, 0.1]);
        // 120-min job but only 90 min until the deadline → must start now.
        let p = plan(
            &spec(Policy::CheapestWindow, 120, Some(base() + Duration::minutes(90))),
            &s,
            base(),
        );
        assert!(p.run_now && p.forced);
    }

    #[test]
    fn threshold_runs_below_and_waits_above() {
        let s = series(base(), &[1.0, 0.3]);
        let job = spec(Policy::Threshold { max_nok_per_kwh: 0.5 }, 60, None);

        let p0 = plan(&job, &s, base());
        assert!(!p0.run_now);
        assert_eq!(p0.start_at, Some(base() + Duration::hours(1)));

        let p1 = plan(&job, &s, base() + Duration::hours(1));
        assert!(p1.run_now);
    }

    #[test]
    fn immediate_always_runs() {
        let s = series(base(), &[9.9]);
        let p = plan(&spec(Policy::Immediate, 60, None), &s, base());
        assert!(p.run_now);
    }

    #[test]
    fn multi_hour_window_needs_enough_slots() {
        // Only 2 hours of data but a 3-hour job and no deadline → wait.
        let s = series(base(), &[0.1, 0.1]);
        let p = plan(&spec(Policy::CheapestWindow, 180, None), &s, base());
        assert!(!p.run_now);
    }
}
