#![allow(unused_imports)]
use copper_core::port::{In, Out};
use copper_core::{Bits, Clock, ClockDomain};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: `o` is written between the leading `step.read()` and the update of
// `r`, the register the write reads. The read's pre-edge barrier drags the write
// to the pre-edge settle, where it captures `r`'s PRE-update value — one
// generation behind what the emitted `assign o = r` shows at every observation
// instant. Measured (the V8 battery): sim `[0,1,2,…]` against the SV's
// `[1,2,3,…]`, silently. D1 exempts this segment precisely because the read
// comb-reaches the update, which is why this is its own rule.
#[hardware(sequential)]
async fn stale_publish(clk: Clock<MainClk>, step: In<Bits<8>, MainClk>, o: Out<Bits<8>, MainClk>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        let s = step.read();
        o.write(r);
        r = r + s;
        clk.tick().await;
    }
}

fn main() {}
