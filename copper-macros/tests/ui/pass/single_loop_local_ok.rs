use copper_core::{Bits, Clock, ClockDomain};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should pass — the boundary case for the two new guardrails: a pre-loop register
// declaration, a single top-level loop as the final statement, and a `let next`
// local whose name does NOT collide with any port. None of this trips the
// single-loop or param-shadowing checks.
//
// REORDERED 2026-08-26: the original ordering (`let next = …read(); out.write(count);
// count = next;`) put the port write between the leading read and the register
// update — the V8a shape, MEASURED divergent (sim `[0,1,…]` vs SV `[1,2,…]`; pinned
// as `v8d_temp_renamed_update` in tests/sequential_forwarding_divergence.rs). This
// fixture was compile-only, so nothing had ever measured it; the
// write-between-read-and-update rule flagged it the day it landed. The write now
// precedes the read (V8c's measured-agreeing form), which preserves everything this
// fixture exists to exercise.
#[hardware(sequential)]
async fn accum(clk: Clock<MainClk>, step: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) {
    let mut count: Bits<8> = Bits::zero();
    loop {
        out.write(count);
        let next = count + step.read();
        count = next;
        clk.tick().await;
    }
}

fn main() {}
