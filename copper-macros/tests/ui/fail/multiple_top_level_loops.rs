use copper_core::{Bits, Clock, ClockDomain};
use copper_core::port::Out;
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: two top-level `loop { … }` statements. A hardware module is a single
// state machine; the second loop is unreachable (the first never breaks). The macro
// requires exactly one top-level loop.
#[hardware(sequential)]
async fn two_loops(clk: Clock<MainClk>, out: Out<Bits<8>, MainClk>) {
    loop {
        out.write(Bits::zero());
        clk.tick().await;
    }
    loop {
        clk.tick().await;
    }
}

fn main() {}
