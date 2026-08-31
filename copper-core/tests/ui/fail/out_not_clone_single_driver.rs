// Migrated from a `compile_fail` doctest in `copper-core/src/port.rs` (2026-08-26).
//
// `Out` is not `Clone`: a wire has exactly one driver, so a second writer is a compile error rather than a run-time race.
//
// WHY IT MOVED: `compile_fail` asserts only that the snippet failed to compile —
// a typo or a renamed API satisfies it just as well as the invariant it names, and
// the error itself was never checked. trybuild pins the compiler's message in the
// adjacent .stderr.

use copper_core::port::wire;
use copper_core::Logic;

fn main() {
    let (out, _in) = wire::<Logic, ()>(Logic::Zero);
    let _second_driver = out.clone(); // error: `Out<Logic>` is not `Clone`
}
