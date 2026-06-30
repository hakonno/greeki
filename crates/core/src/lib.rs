//! Pure, I/O-free scheduling logic for a power-price-aware compute scheduler.
//!
//! The whole point of this crate is to be deterministic and testable: given a
//! price series and a job specification, decide *when* the job should run. It
//! has no notion of databases, HTTP, or the wall clock — `now` is always passed
//! in. That keeps the interesting decisions (which the rest of the system leans
//! on) under unit tests.

pub mod job;
pub mod price;
pub mod schedule;

pub use job::{JobSpec, Policy, Priority};
pub use price::{PricePoint, PriceSeries};
pub use schedule::{cheapest_window, hour_floor, plan, slots_for, window_at, Plan, Window};
