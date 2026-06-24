use copper_core::{Clock, ClockDomain};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: raw parameter type — all params must be Clock<D>, In<T>, or Out<T>
#[hardware(sequential)]
async fn counter(clk: Clock<MainClk>, input: u8, out: Out<u8>) {
    loop {
        out.write(input);
        clk.tick().await;
    }
}

fn main() {}
