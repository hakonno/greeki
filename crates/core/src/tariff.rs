//! Turning a raw spot price into the price a household actually pays.
//!
//! Optimizing the bare Nord Pool spot price is a mistake in Norway: the number
//! on the bill is `spot + grid energy + electricity tax`, all under 25% VAT, and
//! — crucially — the **strømstøtte** support scheme refunds most of the spot
//! price above a threshold, which flattens the expensive end of the curve. A
//! scheduler that ranks hours by raw spot therefore chases a spread the customer
//! barely experiences.
//!
//! This module is a small, pure transform from raw spot (kr/kWh, ex-VAT) to the
//! effective consumer price (kr/kWh, incl-VAT). It is deliberately simple and
//! its assumptions are configurable; it is not a substitute for your actual
//! tariff sheet, but it is far closer to reality than raw spot.

use serde::Deserialize;

/// Components of the electricity price beyond the spot price. All monetary
/// fields are kr/kWh, ex-VAT, except `vat_rate` (a fraction) and `subsidy_rate`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Tariff {
    /// Energy component of the grid rent (nettleie), kr/kWh ex-VAT.
    #[serde(default = "d_grid_energy")]
    pub grid_energy_nok_per_kwh: f64,
    /// Electricity tax (elavgift / forbruksavgift), kr/kWh ex-VAT.
    #[serde(default = "d_elavgift")]
    pub electricity_tax_nok_per_kwh: f64,
    /// Value-added tax (MVA) as a fraction. 0.25 for most of Norway; set 0.0
    /// for the VAT-exempt northern counties.
    #[serde(default = "d_vat")]
    pub vat_rate: f64,
    /// Spot price (ex-VAT) above which strømstøtte begins to refund, kr/kWh.
    #[serde(default = "d_subsidy_threshold")]
    pub subsidy_threshold_nok_per_kwh: f64,
    /// Fraction of the spot price *above* the threshold that is refunded.
    /// Set to 0.0 to model "no support".
    #[serde(default = "d_subsidy_rate")]
    pub subsidy_rate: f64,
}

fn d_grid_energy() -> f64 {
    0.40
}
fn d_elavgift() -> f64 {
    0.1644
}
fn d_vat() -> f64 {
    0.25
}
fn d_subsidy_threshold() -> f64 {
    0.9375
}
fn d_subsidy_rate() -> f64 {
    0.90
}

impl Default for Tariff {
    fn default() -> Self {
        Tariff {
            grid_energy_nok_per_kwh: d_grid_energy(),
            electricity_tax_nok_per_kwh: d_elavgift(),
            vat_rate: d_vat(),
            subsidy_threshold_nok_per_kwh: d_subsidy_threshold(),
            subsidy_rate: d_subsidy_rate(),
        }
    }
}

impl Tariff {
    /// A pass-through tariff: effective price == raw spot. Useful for tests and
    /// for users who genuinely want to optimize bare spot.
    pub fn raw_spot() -> Self {
        Tariff {
            grid_energy_nok_per_kwh: 0.0,
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

    /// The all-in price the customer pays for one kWh consumed in an hour whose
    /// raw spot price is `spot_nok_per_kwh` (ex-VAT). Includes grid energy,
    /// electricity tax, the strømstøtte refund, and VAT on the lot.
    pub fn effective(&self, spot_nok_per_kwh: f64) -> f64 {
        let net_spot = spot_nok_per_kwh - self.subsidy(spot_nok_per_kwh);
        let ex_vat = net_spot + self.grid_energy_nok_per_kwh + self.electricity_tax_nok_per_kwh;
        ex_vat * (1.0 + self.vat_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn raw_spot_is_pass_through() {
        let t = Tariff::raw_spot();
        approx(t.effective(0.5), 0.5);
        approx(t.effective(3.0), 3.0);
        approx(t.subsidy(10.0), 0.0);
    }

    #[test]
    fn adds_grid_tax_and_vat_below_threshold() {
        let t = Tariff::default();
        // 0.50 spot, no subsidy: (0.50 + 0.40 + 0.1644) * 1.25
        approx(t.effective(0.50), (0.50 + 0.40 + 0.1644) * 1.25);
        approx(t.subsidy(0.50), 0.0);
    }

    #[test]
    fn subsidy_compresses_the_expensive_end() {
        let t = Tariff::default();
        // At 2.0 kr spot, 90% of (2.0 - 0.9375) is refunded.
        let expected_subsidy = 0.90 * (2.0 - 0.9375);
        approx(t.subsidy(2.0), expected_subsidy);
        let net = 2.0 - expected_subsidy;
        approx(t.effective(2.0), (net + 0.40 + 0.1644) * 1.25);
    }

    #[test]
    fn subsidy_shrinks_the_spread_seen_by_the_customer() {
        let t = Tariff::default();
        // A 2.5 kr raw spread between a cheap and an expensive hour...
        let cheap_spot = 0.20;
        let pricey_spot = 2.70;
        let raw_spread = pricey_spot - cheap_spot;
        let eff_spread = t.effective(pricey_spot) - t.effective(cheap_spot);
        // ...is much smaller after support, even with VAT scaling it up.
        assert!(
            eff_spread < raw_spread,
            "effective spread {eff_spread} should be below raw {raw_spread}"
        );
    }

    #[test]
    fn effective_is_monotonic_in_spot() {
        let t = Tariff::default();
        let mut prev = f64::NEG_INFINITY;
        let mut s = 0.0;
        while s < 5.0 {
            let e = t.effective(s);
            assert!(e >= prev, "effective price dipped at spot {s}");
            prev = e;
            s += 0.05;
        }
    }
}
