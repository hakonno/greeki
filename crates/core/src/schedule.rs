//! The scheduling decision: given a job spec, a price series and the current
//! time, decide whether the job should start *now* and, if not, when the
//! cheapest opportunity is. Window search lives in [`crate::window`].
//!
//! Design note — we never commit to a far-future plan. The price API only knows
//! ~24–48h ahead, so every scheduler tick re-runs `plan` against the latest
//! known prices. A job "waits" simply by not being told to run yet; once a
//! cheaper hour becomes the current hour (or new prices reveal a better
//! window), the next tick picks it up. This keeps the logic stateless and
//! robust to the limited price horizon.

use chrono::{DateTime, Duration, Utc};

use crate::job::{JobSpec, Policy};
use crate::price::PriceSeries;
use crate::window::{cheapest_window, hour_floor, slots_for};

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

/// Decide whether and when `spec` should run, given `prices` and the current
/// time `now`.
pub fn plan(spec: &JobSpec, prices: &PriceSeries, now: DateTime<Utc>) -> Plan {
    let duration = Duration::minutes(spec.duration_minutes.max(0));
    match spec.policy {
        Policy::Immediate if before_earliest(spec, now) => Plan {
            run_now: false,
            start_at: spec.earliest_start,
            forced: false,
            window_avg_nok: None,
            reason: "waiting for its earliest-start time".to_string(),
        },
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

/// True while `spec.earliest_start` still forbids starting. Nothing — not even
/// a deadline — overrides it; a deadline earlier than the earliest start is a
/// contradiction and the earliest start wins.
fn before_earliest(spec: &JobSpec, now: DateTime<Utc>) -> bool {
    matches!(spec.earliest_start, Some(e) if now < e)
}

fn plan_threshold(
    spec: &JobSpec,
    prices: &PriceSeries,
    now: DateTime<Utc>,
    max: f64,
    duration: Duration,
) -> Plan {
    let gated = before_earliest(spec, now);
    if deadline_forces(spec.deadline, now, duration) && !gated {
        return Plan {
            run_now: true,
            start_at: Some(now),
            forced: true,
            window_avg_nok: prices.price_at(now).map(|p| p.nok_per_kwh),
            reason: "deadline reached – starting despite price".to_string(),
        };
    }

    match prices.price_at(now) {
        Some(p) if p.nok_per_kwh <= max && !gated => Plan {
            run_now: true,
            start_at: Some(now),
            forced: false,
            window_avg_nok: Some(p.nok_per_kwh),
            reason: format!("price {:.3} kr ≤ threshold {:.3} kr", p.nok_per_kwh, max),
        },
        Some(p) => {
            let not_before = spec.earliest_start.unwrap_or(now);
            // First hour that is not over, not entirely before the allowed
            // start, and satisfies the threshold and deadline. The projected
            // start clamps into that hour if the gate opens mid-hour.
            let next = prices
                .points
                .iter()
                .find(|q| {
                    q.end > now
                        && q.end > not_before
                        && q.nok_per_kwh <= max
                        && spec.deadline.map_or(true, |dl| q.end <= dl)
                })
                .map(|q| q.start.max(not_before).max(now));
            let reason = if gated && p.nok_per_kwh <= max {
                format!(
                    "price {:.3} kr is fine – waiting for its earliest-start time",
                    p.nok_per_kwh
                )
            } else {
                format!("price {:.3} kr > threshold {:.3} kr – waiting", p.nok_per_kwh, max)
            };
            Plan {
                run_now: false,
                start_at: next,
                forced: false,
                window_avg_nok: Some(p.nok_per_kwh),
                reason,
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
    // Windows may not start before the current hour, nor before an explicit
    // earliest start. The earliest start is deliberately *not* floored: a
    // window whose hour begins before it would allow a too-early launch.
    let earliest = match spec.earliest_start {
        Some(e) if e > hour_floor(now) => e,
        _ => hour_floor(now),
    };
    let forced = deadline_forces(spec.deadline, now, duration) && !before_earliest(spec, now);

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
    use crate::testutil::{base, series};

    fn spec(policy: Policy, minutes: i64, deadline: Option<DateTime<Utc>>) -> JobSpec {
        JobSpec {
            policy,
            duration_minutes: minutes,
            deadline,
            earliest_start: None,
        }
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

    // -- earliest_start: a recurring job's next occurrence must not fire the
    //    same day it was created, no matter how cheap the prices are. --

    #[test]
    fn earliest_start_holds_back_cheapest_window() {
        // Cheapest hour is right now, but the job may not start until hour 2.
        let s = series(base(), &[0.1, 0.5, 0.2, 0.9]);
        let mut job = spec(Policy::CheapestWindow, 60, None);
        job.earliest_start = Some(base() + Duration::hours(2));

        let p = plan(&job, &s, base());
        assert!(!p.run_now);
        assert_eq!(p.start_at, Some(base() + Duration::hours(2)));

        let p2 = plan(&job, &s, base() + Duration::hours(2));
        assert!(p2.run_now);
    }

    #[test]
    fn earliest_start_holds_back_threshold() {
        // Price is below the threshold the whole time; only the gate decides.
        let s = series(base(), &[0.1, 0.1, 0.1]);
        let mut job = spec(Policy::Threshold { max_nok_per_kwh: 0.5 }, 60, None);
        job.earliest_start = Some(base() + Duration::hours(1));

        let p0 = plan(&job, &s, base());
        assert!(!p0.run_now);
        assert_eq!(p0.start_at, Some(base() + Duration::hours(1)));

        let p1 = plan(&job, &s, base() + Duration::hours(1));
        assert!(p1.run_now);
    }

    #[test]
    fn earliest_start_holds_back_immediate() {
        let s = series(base(), &[0.1]);
        let mut job = spec(Policy::Immediate, 60, None);
        job.earliest_start = Some(base() + Duration::hours(2));

        assert!(!plan(&job, &s, base()).run_now);
        assert!(plan(&job, &s, base() + Duration::hours(2)).run_now);
    }

    #[test]
    fn deadline_does_not_force_past_earliest_start() {
        // Deadline math says "must start now to finish", but the earliest
        // start forbids it — the gate wins over the (contradictory) deadline.
        let s = series(base(), &[0.5, 0.5]);
        let mut job = spec(
            Policy::CheapestWindow,
            120,
            Some(base() + Duration::minutes(90)),
        );
        job.earliest_start = Some(base() + Duration::hours(1));

        let p = plan(&job, &s, base());
        assert!(!p.run_now);
    }

    #[test]
    fn mid_hour_earliest_start_delays_to_next_full_hour_window() {
        // Earliest start 00:30: the 00:00 window would begin before it, so the
        // first eligible cheapest-window start is 01:00.
        let s = series(base(), &[0.1, 0.2, 0.9]);
        let mut job = spec(Policy::CheapestWindow, 60, None);
        job.earliest_start = Some(base() + Duration::minutes(30));

        let p = plan(&job, &s, base() + Duration::minutes(45));
        assert!(!p.run_now);
        assert_eq!(p.start_at, Some(base() + Duration::hours(1)));
    }
}
