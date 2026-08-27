// CDC guarantee, migrated from a `compile_fail` doctest in
// `copper-core/src/cdc.rs` (2026-08-26).
//
// Passing a `Fast` clock where a `Slow` clock is required:
//
// WHY IT MOVED: a `compile_fail` doctest asserts only that the code failed to
// compile — a typo, a renamed API or a missing import satisfies it exactly as
// well as the guarantee it is meant to state. trybuild pins the compiler's own
// message in the adjacent .stderr, so this asserts WHY it fails. CLAUDE.md calls
// the CDC rules an executable specification; this is what makes that true.

use copper_core::{Clock, ClockDomain};

struct Fast; impl ClockDomain for Fast {}
struct Slow; impl ClockDomain for Slow {}
fn slow_module(_c: Clock<Slow>) {}

fn main() {
    slow_module(Clock::<Fast>::new()); // E0308: expected Clock<Slow>, found Clock<Fast>
}
