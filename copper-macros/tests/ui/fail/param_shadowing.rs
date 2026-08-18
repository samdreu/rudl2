use copper_core::{Bits, Clock, ClockDomain};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: the `let d` binding shadows the hardware input port `d`. After the
// shadow, `d.read()` would hit the local `Bits`, not the wire — the port is hidden
// and the stimulus never reaches the logic. The macro rejects the shadow.
#[hardware(sequential)]
async fn shadow(clk: Clock<MainClk>, d: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) {
    loop {
        let d: Bits<8> = Bits::zero();
        out.write(d);
        clk.tick().await;
    }
}

fn main() {}
