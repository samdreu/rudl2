use copper_core::{Clock, ClockDomain, Bits};
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

// For now, without macro - just async fn
async fn counter(clk: Clock<MainClk>) {
    let mut count = Bits::<8>::from_u128(0);
    loop {
        println!("count = {}", count.as_u128());
        clk.tick().await;
        count = count + Bits::<8>::from_u128(1);
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    
    exec.spawn(counter(clk.clone()));
    
    for _ in 0..5 {
        exec.tick_clock(&mut clk);
    }
}
