use copper_core::{Bit, Bits, Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, emit};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(function_typed)]
async fn <N>synchronous_fifo(
    clk: Clock<MainClk>,
    din: Arc<Mutex<Bits<N>>>,
    wr_en: Arc<Mutex<Bit>>,
    rd_en: Arc<Mutex<Bit>>,
) -> (Arc<Mutex<(Bits<N>, Bit, Bit, Bit)>>) {
)
