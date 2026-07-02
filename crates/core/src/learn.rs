//! Turning measured run history into planning estimates.
//!
//! We can directly measure how long a job actually ran (finish − start), so we
//! learn a job's runtime from its own history instead of trusting a one-off
//! guess. The estimate is biased slightly long on purpose: under-estimating a
//! deadline-bound job's duration risks starting too late and missing the
//! deadline, which is worse than reserving a little extra cheap time.
//!
//! Power is deliberately *not* learned here: spotwatt has no power meter, so
//! `power_watts` stays a user estimate. Wire up a smart plug or node power
//! sensor later and the same instance-from-history pattern applies.

/// Extra headroom added to the mean so consistent jobs still get a small buffer.
const SAFETY_MARGIN: f64 = 1.15;

/// Estimate planning minutes from recent measured run durations (in minutes).
///
/// Returns `None` until at least `min_samples` positive samples exist. The
/// estimate is the larger of (mean × safety margin) and the longest observed
/// run, so it never under-covers a duration we've actually seen.
pub fn estimate_minutes(samples: &[i64], min_samples: usize) -> Option<i64> {
    if samples.len() < min_samples.max(1) {
        return None;
    }
    // Every sample is a real measured run (the caller only passes completed
    // runs). Clamp clock-skew negatives to zero; a very fast run is a legitimate
    // 0-minute sample and must still count toward the history.
    let clean: Vec<i64> = samples.iter().map(|&m| m.max(0)).collect();
    let mean = clean.iter().sum::<i64>() as f64 / clean.len() as f64;
    let padded = (mean * SAFETY_MARGIN).ceil() as i64;
    let longest = *clean.iter().max().unwrap();
    // Floor at 1: the scheduler plans in whole minutes / hourly slots, so even a
    // sub-minute command occupies at least one minute.
    Some(padded.max(longest).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_until_enough_samples() {
        assert_eq!(estimate_minutes(&[40, 41], 3), None);
        assert!(estimate_minutes(&[40, 41, 42], 3).is_some());
    }

    #[test]
    fn counts_fast_sub_minute_runs() {
        // Three quick commands (each rounds to 0 min) still produce a usable,
        // floored estimate — they must not be silently dropped.
        assert_eq!(estimate_minutes(&[0, 0, 0], 3), Some(1));
    }

    #[test]
    fn consistent_runs_get_margin() {
        // mean 40 × 1.15 = 46, which exceeds the longest run (40).
        assert_eq!(estimate_minutes(&[40, 40, 40], 3), Some(46));
    }

    #[test]
    fn never_under_covers_the_longest_run() {
        // mean 50 × 1.15 = 57.5 → 58, but one run took 90, so estimate is 90.
        assert_eq!(estimate_minutes(&[30, 30, 90], 3), Some(90));
    }
}
