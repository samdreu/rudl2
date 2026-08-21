use copper_core::{Clock, ClockDomain, Logic};
use copper_core::port::{In, RegOut};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should pass: the same multi-write-around-a-tick body is fine on a `RegOut` — a
// registered (non-blocking) output buffers and commits at the edge, so it does not
// collapse and matches the transpiled `always_ff`.
#[hardware(sequential)]
async fn ok(clk: Clock<MainClk>, sel: In<Logic, MainClk>, out: RegOut<Logic, MainClk>) {
    loop {
        let _s = sel.read();
        out.write(Logic::Zero);
        clk.tick().await;
        out.write(Logic::One);
        clk.tick().await;
    }
}

fn main() {}
