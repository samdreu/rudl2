use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// This should fail: function_typed doesn't allow `mut` bindings on inputs
#[hardware(function_typed)]
async fn bad_module(clk: Clock<MainClk>, mut input: u8) -> u8 {
    loop {
        input = input + 1;  // Error: can't mutate input
        clk.tick().await;
    }
}

fn main() {}
