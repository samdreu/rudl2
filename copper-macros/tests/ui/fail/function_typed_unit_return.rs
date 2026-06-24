use copper_core::{Clock, ClockDomain};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: hardware functions must not have a return value
#[hardware(sequential)]
async fn counter(clk: Clock<MainClk>, input: In<u8>, out: Out<u8>) -> u8 {
    loop {
        out.write(input.read());
        clk.tick().await;
    }
}

fn main() {}
