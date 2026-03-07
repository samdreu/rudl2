use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// This should fail: function_typed doesn't allow unit return type
#[hardware(function_typed)]
async fn bad_module(clk: Clock<MainClk>, input: u8) -> () {
    loop {
        clk.tick().await;
    }
}

fn main() {}
