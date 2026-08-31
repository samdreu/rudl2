use copper_core::{Clock, ClockDomain, Logic};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: `out` is written on both sides of a bare `clk.tick().await` within one
// iteration, after a leading `sel` read. The simulator collapses this
// multi-write-around-a-tick to the last write (silent sim ≠ synth). The macro rejects
// it, pointing at `RegOut`.
#[hardware(sequential)]
async fn collapser(clk: Clock<MainClk>, sel: In<Logic, MainClk>, out: Out<Logic, MainClk>) {
    loop {
        let _s = sel.read();
        out.write(Logic::Zero);
        clk.tick().await;
        out.write(Logic::One);
        clk.tick().await;
    }
}

fn main() {}
