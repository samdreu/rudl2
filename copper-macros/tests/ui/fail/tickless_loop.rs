use copper_core::{Bits, Clock, ClockDomain};
use copper_core::port::Out;
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: the loop body contains no `clk.tick().await` at all, so it spins
// in zero time — a combinational cycle. The macro rejects a tick-free sequential
// body with a spanned error.
#[hardware(sequential)]
async fn spinner(clk: Clock<MainClk>, out: Out<Bits<8>, MainClk>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        out.write(n);
        n = n + Bits::from_lit::<1>();
    }
}

fn main() {}
