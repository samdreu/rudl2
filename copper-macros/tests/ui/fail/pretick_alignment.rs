#![allow(unused_imports)]
use copper_core::port::Out;
use copper_core::{Bits, Clock, ClockDomain};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: `r` is a register assigned in the pre-tick segment with no input read
// to pin that segment's clock phase, and `o` is a plain combinational `Out` driven
// from it. With no leading read there is no `pre_edge_barrier`, so the segment runs
// in the previous tick's post-edge settle and the observation is a cycle early —
// measured as sim `[2,3,4,…]` against the transpiled SV's `[1,2,3,…]`.
// The macro rejects it, pointing at `RegOut` or a post-tick update.
#[hardware(sequential)]
async fn misaligned(clk: Clock<MainClk>, o: Out<Bits<8>, MainClk>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(r);
        clk.tick().await;
    }
}

fn main() {}
