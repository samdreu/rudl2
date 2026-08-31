use copper_core::{Bits, Clock, ClockDomain};
use copper_core::port::Out;
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: a hardware module may not construct a clock — clocks are provided as
// parameters. A fabricated clock is never driven, so the module would hang. The macro
// rejects the `Clock::new()` in the body.
#[hardware(sequential)]
async fn maker(clk: Clock<MainClk>, out: Out<Bits<8>, MainClk>) {
    let _fabricated = Clock::<MainClk>::new();
    loop {
        out.write(Bits::zero());
        clk.tick().await;
    }
}

fn main() {}
