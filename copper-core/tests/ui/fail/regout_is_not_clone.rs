// CDC guarantee, migrated from a `compile_fail` doctest in
// `copper-core/src/cdc.rs` (2026-08-26).
//
// `RegOut` carries the same single-driver guarantee as `Out`:
//
// WHY IT MOVED: a `compile_fail` doctest asserts only that the code failed to
// compile — a typo, a renamed API or a missing import satisfies it exactly as
// well as the guarantee it is meant to state. trybuild pins the compiler's own
// message in the adjacent .stderr, so this asserts WHY it fails. CLAUDE.md calls
// the CDC rules an executable specification; this is what makes that true.

use copper_core::port::registered_wire;
use copper_core::{Clock, ClockDomain, Logic};

struct Fast; impl ClockDomain for Fast {}

fn main() {
    let clk = Clock::<Fast>::new();
    let (regout, _obs) = registered_wire::<Logic, Fast>(&clk, Logic::Zero);
    let _second = regout.clone(); // error: `RegOut<Logic, Fast>` is not `Clone`
}
