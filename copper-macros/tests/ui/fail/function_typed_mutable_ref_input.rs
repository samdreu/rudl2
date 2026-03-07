use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// This should fail: function_typed doesn't allow mutable reference inputs
#[hardware(function_typed)]
async fn bad_module(clk: Clock<MainClk>, input: &mut u8) -> u8 {
    loop {
        *input = *input + 1;  // Error: can't use &mut ref
        clk.tick().await;
    }
}

fn main() {}
