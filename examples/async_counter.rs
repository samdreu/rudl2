use copper_core::{Clock, ClockDomain, Bits};
use copper_sim::{HardwareExecutor, emit};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(function_typed)]
async fn counter(clk: Clock<MainClk>) -> Bits<8> {
    let mut count = Bits::<8>::from_u128(0);
    
    loop {
        emit!(count.clone());
        clk.tick().await;
        count = count + Bits::<8>::from_u128(1);
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    
    let output = exec.spawn_function_typed(Bits::<8>::from_u128(0), counter(clk.clone()));
    
    for _ in 0..5 {
        exec.tick_clock(&mut clk);
        let val = output.lock().unwrap().as_u128();
        println!("cycle {} count {}", clk.cycle(), val);
    }
}
