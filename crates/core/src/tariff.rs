//! Turning a raw spot price into the price a household actually pays.
//!
//! Optimizing the bare Nord Pool spot price is a mistake in Norway: the number
//! on the bill is `energy + grid energy + electricity tax`, all under 25% VAT,
//! and what "energy" costs depends on the deal:
//!
//! - **Spot + strømstøtte** (the default): the hourly spot price, with the
//!   state refunding most of it above a threshold — which flattens the
//!   expensive end of the curve. A scheduler that ranks hours by raw spot
//!   chases a spread the customer barely experiences.
//! - **Norgespris** (opt-in since October 2025): a fixed state-set price per
//!   kWh replaces spot entirely, and with it strømstøtte. For these households
//!   the spot curve is irrelevant; the only per-hour variation left on the
//!   bill is the grid rent's day/night step. (The scheme's monthly kWh cap is
//!   not modeled — homelab-scale loads stay far below it.)
//!
//! The grid rent's energy component (nettleie energiledd) is itself
//! time-differentiated with most Norwegian grid operators: a cheaper rate at
//! night (22:00–06:00 local) and on weekends.
//!
//! This module is a small, pure transform from raw spot (kr/kWh, ex-VAT) plus
//! the hour to the effective consumer price (kr/kWh, incl-VAT). It is
//! deliberately simple and its assumptions are configurable; it is not a
//! substitute for your actual tariff sheet, but it is far closer to reality
//! than raw spot.

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
use chrono_tz::Europe::Oslo;
use serde::Deserialize;

/// Which deal prices the energy itself (the "strøm" line of the bill).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnergyModel {
    /// Hourly spot price, softened by the strømstøtte refund above a threshold.
    Spot,
    /// Norgespris: a fixed price per kWh regardless of the spot market.
    Norgespris,
}

/// Components of the electricity price beyond the market. All monetary fields
/// are kr/kWh, ex-VAT, except `vat_rate` (a fraction) and `subsidy_rate`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Tariff {
    /// How the energy itself is priced: `spot` (default) or `norgespris`.
    #[serde(default = "d_model")]
    pub energy_model: EnergyModel,
    /// The Norgespris fixed energy price, kr/kWh ex-VAT (0.40 ex-VAT is the
    /// advertised 0.50 incl-VAT). Only used when `energy_model = "norgespris"`.
    #[serde(default = "d_norgespris")]
    pub norgespris_nok_per_kwh: f64,
    /// Energy component of the grid rent (nettleie) on weekdays 06:00–22:00
    /// local time, kr/kWh ex-VAT. The old `grid_energy_nok_per_kwh` config
    /// key still maps here.
    #[serde(default = "d_grid_day", alias = "grid_energy_nok_per_kwh")]
    pub grid_energy_day_nok_per_kwh: f64,
    /// Grid-rent energy component at night (22:00–06:00) and on weekends,
    /// kr/kWh ex-VAT.
    #[serde(default = "d_grid_night")]
    pub grid_energy_night_nok_per_kwh: f64,
    /// Electricity tax (elavgift / forbruksavgift), kr/kWh ex-VAT. The rate is
    /// set in the state budget and changes yearly (0.0713 flat for 2026; 2025
    /// was 0.1644 with a reduced winter rate).
    #[serde(default = "d_elavgift")]
    pub electricity_tax_nok_per_kwh: f64,
    /// Value-added tax (MVA) as a fraction. 0.25 for most of Norway; set 0.0
    /// for the VAT-exempt northern counties.
    #[serde(default = "d_vat")]
    pub vat_rate: f64,
    /// Spot price (ex-VAT) above which strømstøtte begins to refund, kr/kWh.
    /// Adjusted yearly: 0.77 for 2026 (96.25 øre incl-VAT), up from 0.75 in
    /// 2025. Ignored under Norgespris.
    #[serde(default = "d_subsidy_threshold")]
    pub subsidy_threshold_nok_per_kwh: f64,
    /// Fraction of the spot price *above* the threshold that is refunded.
    /// Set to 0.0 to model "no support". Ignored under Norgespris.
    #[serde(default = "d_subsidy_rate")]
    pub subsidy_rate: f64,
}

fn d_model() -> EnergyModel {
    EnergyModel::Spot
}
fn d_norgespris() -> f64 {
    0.40
}
fn d_grid_day() -> f64 {
    0.40
}
fn d_grid_night() -> f64 {
    0.32
}
fn d_elavgift() -> f64 {
    0.0713
}
fn d_vat() -> f64 {
    0.25
}
fn d_subsidy_threshold() -> f64 {
    0.77
}
fn d_subsidy_rate() -> f64 {
    0.90
}

impl Default for Tariff {
    fn default() -> Self {
        Tariff {
            energy_model: d_model(),
            norgespris_nok_per_kwh: d_norgespris(),
            grid_energy_day_nok_per_kwh: d_grid_day(),
            grid_energy_night_nok_per_kwh: d_grid_night(),
            electricity_tax_nok_per_kwh: d_elavgift(),
            vat_rate: d_vat(),
            subsidy_threshold_nok_per_kwh: d_subsidy_threshold(),
            subsidy_rate: d_subsidy_rate(),
        }
    }
}

/// True during the grid operators' cheap-rate hours: nights (22:00–06:00) and
/// all of Saturday/Sunday, in Norwegian local time.
fn night_or_weekend(at: DateTime<Utc>) -> bool {
    let local = at.with_timezone(&Oslo);
    let hour = local.hour();
    matches!(local.weekday(), Weekday::Sat | Weekday::Sun) || hour < 6 || hour >= 22
}

impl Tariff {
    /// A pass-through tariff: effective price == raw spot. Useful for tests and
    /// for users who genuinely want to optimize bare spot.
    pub fn raw_spot() -> Self {
        Tariff {
            energy_model: EnergyModel::Spot,
            norgespris_nok_per_kwh: 0.0,
            grid_energy_day_nok_per_kwh: 0.0,
            grid_energy_night_nok_per_kwh: 0.0,
            electricity_tax_nok_per_kwh: 0.0,
            vat_rate: 0.0,
            subsidy_threshold_nok_per_kwh: f64::INFINITY,
            subsidy_rate: 0.0,
        }
    }

    /// The strømstøtte refund applied to a given raw spot price (kr/kWh, ex-VAT).
    /// Zero at or below the threshold; `rate * (spot - threshold)` above it.
    pub fn subsidy(&self, spot_nok_per_kwh: f64) -> f64 {
        let over = spot_nok_per_kwh - self.subsidy_threshold_nok_per_kwh;
        if over > 0.0 {
            over * self.subsidy_rate
        } else {
            0.0
        }
    }

    /// The grid-rent energy rate in force at `at` (kr/kWh, ex-VAT).
    pub fn grid_energy_at(&self, at: DateTime<Utc>) -> f64 {
        if night_or_weekend(at) {
            self.grid_energy_night_nok_per_kwh
        } else {
            self.grid_energy_day_nok_per_kwh
        }
    }

    /// The all-in price the customer pays for one kWh consumed in the hour
    /// starting at `at`, given that hour's raw spot price (ex-VAT). Includes
    /// the energy model (spot − strømstøtte, or Norgespris), the grid energy
    /// rate in force at that time, electricity tax, and VAT on the lot.
    pub fn effective_at(&self, spot_nok_per_kwh: f64, at: DateTime<Utc>) -> f64 {
        let energy = match self.energy_model {
            EnergyModel::Spot => spot_nok_per_kwh - self.subsidy(spot_nok_per_kwh),
            EnergyModel::Norgespris => self.norgespris_nok_per_kwh,
        };
        let ex_vat = energy + self.grid_energy_at(at) + self.electricity_tax_nok_per_kwh;
        ex_vat * (1.0 + self.vat_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    /// Wednesday 2026-07-01 12:00 Oslo (CEST) — a weekday daytime hour.
    fn weekday_noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 1, 10, 0, 0).unwrap()
    }

    /// Wednesday 2026-07-01 23:00 Oslo — a weekday night hour.
    fn weekday_night() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 1, 21, 0, 0).unwrap()
    }

    /// Saturday 2026-07-04 12:00 Oslo — weekend, midday.
    fn saturday_noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 4, 10, 0, 0).unwrap()
    }

    #[test]
    fn raw_spot_is_pass_through() {
        let t = Tariff::raw_spot();
        approx(t.effective_at(0.5, weekday_noon()), 0.5);
        approx(t.effective_at(3.0, weekday_night()), 3.0);
        approx(t.subsidy(10.0), 0.0);
    }

    #[test]
    fn adds_grid_tax_and_vat_below_threshold() {
        let t = Tariff::default();
        // 0.50 spot, daytime, no subsidy: (0.50 + 0.40 + 0.0713) * 1.25
        approx(
            t.effective_at(0.50, weekday_noon()),
            (0.50 + 0.40 + 0.0713) * 1.25,
        );
        approx(t.subsidy(0.50), 0.0);
    }

    #[test]
    fn night_and_weekend_use_the_cheap_grid_rate() {
        let t = Tariff::default();
        let night = (0.50 + 0.32 + 0.0713) * 1.25;
        approx(t.effective_at(0.50, weekday_night()), night);
        approx(t.effective_at(0.50, saturday_noon()), night);
    }

    #[test]
    fn subsidy_compresses_the_expensive_end() {
        let t = Tariff::default();
        // At 2.0 kr spot, 90% of (2.0 - 0.77) is refunded.
        let expected_subsidy = 0.90 * (2.0 - 0.77);
        approx(t.subsidy(2.0), expected_subsidy);
        let net = 2.0 - expected_subsidy;
        approx(
            t.effective_at(2.0, weekday_noon()),
            (net + 0.40 + 0.0713) * 1.25,
        );
    }

    #[test]
    fn subsidy_shrinks_the_spread_seen_by_the_customer() {
        let t = Tariff::default();
        // A 2.5 kr raw spread between a cheap and an expensive hour...
        let cheap_spot = 0.20;
        let pricey_spot = 2.70;
        let raw_spread = pricey_spot - cheap_spot;
        let eff_spread =
            t.effective_at(pricey_spot, weekday_noon()) - t.effective_at(cheap_spot, weekday_noon());
        // ...is much smaller after support, even with VAT scaling it up.
        assert!(
            eff_spread < raw_spread,
            "effective spread {eff_spread} should be below raw {raw_spread}"
        );
    }

    #[test]
    fn norgespris_ignores_spot_entirely() {
        let t = Tariff {
            energy_model: EnergyModel::Norgespris,
            ..Tariff::default()
        };
        let expected = (0.40 + 0.40 + 0.0713) * 1.25;
        approx(t.effective_at(0.05, weekday_noon()), expected);
        approx(t.effective_at(5.00, weekday_noon()), expected);
    }

    #[test]
    fn norgespris_still_steps_on_the_grid_day_night_rate() {
        let t = Tariff {
            energy_model: EnergyModel::Norgespris,
            ..Tariff::default()
        };
        let day = t.effective_at(1.0, weekday_noon());
        let night = t.effective_at(1.0, weekday_night());
        approx(day - night, (0.40 - 0.32) * 1.25);
    }

    #[test]
    fn effective_is_monotonic_in_spot() {
        let t = Tariff::default();
        let at = weekday_noon();
        let mut prev = f64::NEG_INFINITY;
        let mut s = 0.0;
        while s < 5.0 {
            let e = t.effective_at(s, at);
            assert!(e >= prev, "effective price dipped at spot {s}");
            prev = e;
            s += 0.05;
        }
    }
}
