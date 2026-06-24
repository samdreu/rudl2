use copper_core::{Clock, ClockDomain, Bits};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should pass: valid sequential module
#[hardware(sequential)]
async fn counter(clk: Clock<MainClk>, input: In<Bits<8>>, out: Out<Bits<8>>) {
    let mut count = Bits::<8>::from_lit::<0>();
    loop {
        out.write(count);
        clk.tick().await;
        count = count + input.read();
    }
}

fn main() {}
