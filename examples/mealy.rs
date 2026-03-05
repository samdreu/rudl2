use copper_core::{Bit, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::{verify_with_verilator, HardwareExecutor, SimulationTrace};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[derive(Clone, Copy, Debug)]
enum State {
    S0, // no match
    S1, // saw '1'
    S2, // saw '10'
}

// Mealy machine: detects "101". Output is 1 on the cycle the last '1' arrives.
#[hardware]
async fn mealy_101(
    clk: Clock<MainClk>,
    in_bit: Arc<Mutex<Bit>>,
    out_bit: Arc<Mutex<Bit>>,
) {
    let mut state = State::S0;

    loop {
        let input = *in_bit.lock().unwrap();

        let output = match (state, input.0) {
            (State::S2, Logic::One) => Bit::ONE,
            (_, Logic::X) => Bit::X,
            _ => Bit::ZERO,
        };
        out_bit.lock().unwrap().clone_from(&output);

        clk.tick().await;

        state = match (state, input.0) {
            (State::S0, Logic::One) => State::S1,
            (State::S0, Logic::Zero) => State::S0,
            (State::S1, Logic::Zero) => State::S2,
            (State::S1, Logic::One) => State::S1,
            (State::S2, Logic::One) => State::S1,
            (State::S2, Logic::Zero) => State::S0,
            (_, Logic::X) => state,
        };
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let in_bit = Arc::new(Mutex::new(Bit::ZERO));
    let out_bit = Arc::new(Mutex::new(Bit::ZERO));

    exec.spawn(mealy_101(clk.clone(), Arc::clone(&in_bit), Arc::clone(&out_bit)));

    let inputs = vec![
        Logic::Zero,
        Logic::One,
        Logic::Zero,
        Logic::One, // detects 101 here
        Logic::One,
        Logic::Zero,
        Logic::One, // detects 101 here
        Logic::Zero,
        Logic::Zero,
    ];

    let mut trace = SimulationTrace::new();

    println!("=== Mealy 101 Detector (Rust Simulation) ===");
    for &bit in inputs.iter() {
        *in_bit.lock().unwrap() = Bit(bit);
        exec.tick_clock(&mut clk);
        let out = *out_bit.lock().unwrap();
        println!("cycle {} in={:?} out={:?}", clk.cycle(), bit, out.0);

        trace.add_cycle(
            clk.cycle() as usize,
            vec![("in_bit".to_string(), vec![bit])],
            vec![("out_bit".to_string(), vec![out.0])],
        );
    }

    println!("\n=== Cross-Validating with Verilator ===");
    match verify_with_verilator("verilog/mealy.v", "mealy", &trace) {
        Ok(true) => println!("✓ Verilator verification PASSED! Rust and Verilog match!"),
        Ok(false) => println!("✗ Verilator verification FAILED!"),
        Err(e) => println!("⚠ Verilator verification error: {}", e),
    }
}
