//! Copper HDL — top-level crate.
//!
//! Hosts the hardware standard library: reusable `#[hardware]` modules that live
//! *downstream* of `copper-sim` (the `#[hardware]` macro expands to
//! `::copper_sim::…` paths, so these cannot live inside `copper-sim` itself).

pub mod sync;
pub use sync::sync_2ff;
