use copper_core::{Clock, ClockDomain, State, Bits, Signal};
use copper_sim::HardwareExecutor;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

async fn counter(clk: Clock<MainClk>, out: Arc<Mutex<Signal<MainClk, Bits<8>>>>) {
    let mut count = State::new(Bits::<8>::from_u128(0));
    loop {
        clk.tick().await;
        let next = count.current_clone() + Bits::<8>::from_u128(1);
        count.set_next(next);
        count.advance();

        out.lock().unwrap().write(count.current_clone());
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let out = Arc::new(Mutex::new(Signal::new(Bits::<8>::from_u128(0))));
    exec.spawn(counter(clk.clone(), Arc::clone(&out)));

    for _ in 0..5 {
        exec.tick_clock(&mut clk);
        let val = out.lock().unwrap().read().as_u128();
        println!("cycle {} count {}", clk.cycle(), val);
    }
}
