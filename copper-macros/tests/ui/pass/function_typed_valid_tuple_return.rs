use copper_core::{Clock, ClockDomain, Bit};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// This should pass: function_typed with tuple return type for multiple outputs
#[hardware(function_typed)]
async fn valid_mealy(clk: Clock<MainClk>, input: Bit) -> (Bit, u8) {
    let mut state = 0u8;
    loop {
        clk.tick().await;
        state = state.wrapping_add(1);
    }
}

fn main() {}
