use copper_core::{Bits, Clock, Logic, ClockDomain};
use copper_core::port::{In, RegOut};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: the only tick in the loop is inside the `for`, and the `for` can
// be left by `break` BEFORE that tick. On a mismatch at bit 0 the outer loop
// returns to its top in zero time — a livelock in simulation. Until 2026-09-02
// the reachability check assumed a `for` always runs to its tick, and this
// module compiled and hung the simulator.
#[hardware(sequential)]
async fn det(
    clk: Clock<MainClk>,
    x: In<Logic, MainClk>,
    pat: In<Bits<3>, MainClk>,
    out: RegOut<Logic, MainClk>,
) {
    loop {
        out.write(Logic::Zero);
        let mut ok = true;
        for i in 0..3 {
            if x.read() != pat.read()[i] {
                ok = false;
                break;
            }
            clk.tick().await;
        }
        if ok {
            out.write(Logic::One);
            clk.tick().await;
        }
    }
}

fn main() {}
