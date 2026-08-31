// CDC guarantee, migrated from a `compile_fail` doctest in
// `copper-core/src/cdc.rs` (2026-08-26).
//
// Count 1 — the wire is already moved into its single driver:
//
// WHY IT MOVED: a `compile_fail` doctest asserts only that the code failed to
// compile — a typo, a renamed API or a missing import satisfies it exactly as
// well as the guarantee it is meant to state. trybuild pins the compiler's own
// message in the adjacent .stderr, so this asserts WHY it fails. CLAUDE.md calls
// the CDC rules an executable specification; this is what makes that true.

use copper_core::port::{wire, Out};
use copper_core::{Clock, ClockDomain, Logic};

struct Fast; impl ClockDomain for Fast {}
fn driver_a(_c: Clock<Fast>, _q: Out<Logic, Fast>) {}
fn driver_b(_c: Clock<Fast>, _q: Out<Logic, Fast>) {}

fn main() {
    let (out, _obs) = wire::<Logic, Fast>(Logic::Zero);
    driver_a(Clock::<Fast>::new(), out);
    // E0382: use of moved value `out` — a wire has one driver, by move semantics
    driver_b(Clock::<Fast>::new(), out);
}
