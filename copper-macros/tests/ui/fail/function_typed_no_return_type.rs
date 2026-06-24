use copper_core::{Clock, ClockDomain};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: sequential must be async
#[hardware(sequential)]
fn counter(clk: Clock<MainClk>, input: In<u8>, out: Out<u8>) {
    loop {}
}

fn main() {}
