//! Pure, I/O-free scheduling logic for a power-price-aware compute scheduler.
//!
//! The whole point of this crate is to be deterministic and testable: given a
//! price series and a job specification, decide *when* the job should run. It
//! has no notion of databases, HTTP, or the wall clock — `now` is always passed
//! in. That keeps the interesting decisions (which the rest of the system leans
//! on) under unit tests.

pub mod cost;
pub mod job;
pub mod learn;
pub mod price;
pub mod schedule;
pub mod select;
pub mod tariff;

pub use cost::interval_cost;
pub use job::{JobSpec, Policy, Priority};
pub use learn::estimate_minutes;
pub use price::{PricePoint, PriceSeries};
pub use schedule::{cheapest_window, hour_floor, plan, slots_for, window_at, Plan, Window};
pub use select::select_within_budget;
pub use tariff::{EnergyModel, Tariff};
