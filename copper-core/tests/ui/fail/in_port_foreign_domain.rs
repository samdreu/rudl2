// CDC guarantee, migrated from a `compile_fail` doctest in
// `copper-core/src/cdc.rs` (2026-08-26).
//
// Connecting a `Fast`-domain input wire to a `Slow`-domain input port:
//
// WHY IT MOVED: a `compile_fail` doctest asserts only that the code failed to
// compile — a typo, a renamed API or a missing import satisfies it exactly as
// well as the guarantee it is meant to state. trybuild pins the compiler's own
// message in the adjacent .stderr, so this asserts WHY it fails. CLAUDE.md calls
// the CDC rules an executable specification; this is what makes that true.

use copper_core::port::{wire, In, Out};
use copper_core::{Clock, ClockDomain, Logic};

struct Fast; impl ClockDomain for Fast {}
struct Slow; impl ClockDomain for Slow {}
fn slow_module(_c: Clock<Slow>, _d: In<Logic, Slow>, _q: Out<Logic, Slow>) {}

fn main() {
    let (_o, fast_in) = wire::<Logic, Fast>(Logic::Zero);
    let (slow_q, _obs) = wire::<Logic, Slow>(Logic::Zero);
    // E0308: expected `In<Logic, Slow>`, found `In<Logic, Fast>`
    slow_module(Clock::<Slow>::new(), fast_in, slow_q);
}
