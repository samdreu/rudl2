use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// This should pass: function_typed with immutable input and return type
#[hardware(function_typed)]
async fn valid_counter(clk: Clock<MainClk>, input: u8) -> u8 {
    let mut count = 0u8;
    loop {
        clk.tick().await;
        count = count.wrapping_add(input);
    }
}

fn main() {}
