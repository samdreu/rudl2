use copper_core::{Bits, Clock, ClockDomain};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should pass — the boundary case for the two new guardrails: a pre-loop register
// declaration, a single top-level loop as the final statement, and a `let next`
// local whose name does NOT collide with any port. None of this trips the
// single-loop or param-shadowing checks.
#[hardware(sequential)]
async fn accum(clk: Clock<MainClk>, step: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) {
    let mut count: Bits<8> = Bits::zero();
    loop {
        let next = count + step.read();
        out.write(count);
        count = next;
        clk.tick().await;
    }
}

fn main() {}
