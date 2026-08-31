use copper_core::{Clock, ClockDomain, Bits};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: `input` is read through a clone (`input2`), which the per-read
// freshness check can't see and protect — #[hardware(sequential)] rejects
// this at compile time rather than silently leaving it unprotected.
#[hardware(sequential)]
async fn counter(clk: Clock<MainClk>, input: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) {
    let mut count = Bits::<8>::from_lit::<0>();
    loop {
        out.write(count);
        clk.tick().await;
        let input2 = input.clone();
        count = count + input2.read();
    }
}

fn main() {}
