//! Copper HDL — top-level crate.
//!
//! Hosts the hardware standard library: reusable `#[hardware]` modules that live
//! *downstream* of `copper-sim` (the `#[hardware]` macro expands to
//! `::copper_sim::…` paths, so these cannot live inside `copper-sim` itself).
//!
//! # Clocks are provided, not constructed
//!
//! A hardware module receives its clock as a parameter and may not create one —
//! a fabricated clock is never driven, so the module would hang (CDC audit,
//! "Gap 2"). This is rejected at compile time:
//!
//! ```compile_fail
//! use copper_core::port::Out;
//! use copper_core::{Clock, ClockDomain, Logic};
//! use copper_macros::hardware;
//! struct Slow; impl ClockDomain for Slow {}
//! struct Fast; impl ClockDomain for Fast {}
//!
//! #[hardware(sequential)]
//! async fn m(clk: Clock<Slow>, q: Out<Logic, Slow>) {
//!     let f = Clock::<Fast>::new();   // rejected: a module may not create a clock
//!     loop { q.write(Logic::Zero); clk.tick().await; let _ = &f; }
//! }
//! ```
//!
//! Cloning a clock parameter to pass to a submodule stays legal — a clone shares
//! the same real clock:
//!
//! ```
//! use copper_core::port::Out;
//! use copper_core::{Clock, ClockDomain, Logic};
//! use copper_macros::hardware;
//! struct Slow; impl ClockDomain for Slow {}
//!
//! #[hardware(sequential)]
//! async fn m(clk: Clock<Slow>, q: Out<Logic, Slow>) {
//!     let child_clk = clk.clone();
//!     loop { q.write(Logic::Zero); clk.tick().await; let _ = &child_clk; }
//! }
//! ```

pub mod sync;
pub use sync::sync_2ff;
