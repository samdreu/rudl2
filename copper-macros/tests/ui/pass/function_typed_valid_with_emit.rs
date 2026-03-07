use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::emit;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// This should pass: function_typed with Arc<Mutex> inputs and emit! output
#[hardware(function_typed)]
async fn valid_pipeline(
    clk: Clock<MainClk>, 
    input: Arc<Mutex<u8>>,
    output: Arc<Mutex<u8>>,
) -> u8 {
    let mut reg = 0u8;
    loop {
        emit!(output, reg);
        clk.tick().await;
        let val = *input.lock().unwrap();
        reg = val.wrapping_add(1);
    }
}

fn main() {}
