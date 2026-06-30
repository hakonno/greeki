//! Choosing *which* due jobs to actually launch this tick.
//!
//! A plain count cap ("at most N jobs at once") limits parallelism but says
//! nothing about power. In Norway the grid bill has a capacity component
//! (kapasitetsledd / effekttariff) keyed to your *peak hourly draw*, so the
//! lever that saves real money is keeping the simultaneous load under a budget —
//! not merely the job count. This module adds that second constraint.
//!
//! It is pure and deterministic: given candidates already ordered by launch
//! priority, the count of free slots, the watts already committed by running
//! jobs, and an optional site power budget, it returns the indices to start.

/// Greedily pick candidates to launch, honoring both a free-slot count and a
/// site power budget. Inputs are the candidates' power draw in watts (use `0.0`
/// when a job's draw is unknown), in launch-priority order.
///
/// A candidate that would push the committed load over `budget_watts` is
/// **skipped, not a hard stop** — a smaller job further down can still use the
/// leftover headroom, and the skipped job gets another chance on a later tick
/// once running jobs free up power. To avoid starving a job whose draw alone
/// exceeds the budget, a candidate is always allowed when nothing is yet
/// committed this evaluation (it's the sole load; running it is the best we can
/// do).
pub fn select_within_budget(
    candidate_watts: &[f64],
    free_slots: usize,
    committed_watts: f64,
    budget_watts: Option<f64>,
) -> Vec<usize> {
    let mut chosen = Vec::new();
    let mut committed = committed_watts;
    for (i, &w) in candidate_watts.iter().enumerate() {
        if chosen.len() >= free_slots {
            break;
        }
        let fits = match budget_watts {
            Some(budget) => committed + w <= budget || committed <= 0.0,
            None => true,
        };
        if fits {
            chosen.push(i);
            committed += w.max(0.0);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_budget_is_slot_limited_only() {
        let got = select_within_budget(&[100.0, 100.0, 100.0], 2, 0.0, None);
        assert_eq!(got, vec![0, 1]);
    }

    #[test]
    fn budget_caps_simultaneous_power() {
        // 3000 W budget, three 2000 W jobs: only the first fits.
        let got = select_within_budget(&[2000.0, 2000.0, 2000.0], 5, 0.0, Some(3000.0));
        assert_eq!(got, vec![0]);
    }

    #[test]
    fn already_committed_power_reduces_headroom() {
        // 3000 W budget, 2500 W already running: a 1000 W job doesn't fit.
        let got = select_within_budget(&[1000.0], 5, 2500.0, Some(3000.0));
        assert!(got.is_empty());
    }

    #[test]
    fn smaller_job_slots_into_leftover_headroom() {
        // 3000 W budget: the 2500 W job fits, the next 2500 W is skipped,
        // but the trailing 400 W job still fits the remaining 500 W.
        let got = select_within_budget(&[2500.0, 2500.0, 400.0], 5, 0.0, Some(3000.0));
        assert_eq!(got, vec![0, 2]);
    }

    #[test]
    fn oversized_job_runs_when_nothing_is_committed() {
        // A 9000 W job alone exceeds the 3000 W budget, but starving it forever
        // is worse — it runs as the sole load.
        let got = select_within_budget(&[9000.0], 5, 0.0, Some(3000.0));
        assert_eq!(got, vec![0]);
    }

    #[test]
    fn oversized_job_waits_if_something_else_is_running() {
        let got = select_within_budget(&[9000.0], 5, 100.0, Some(3000.0));
        assert!(got.is_empty());
    }

    #[test]
    fn unknown_power_is_treated_as_zero_and_always_fits() {
        let got = select_within_budget(&[0.0, 0.0], 5, 2999.0, Some(3000.0));
        assert_eq!(got, vec![0, 1]);
    }
}
