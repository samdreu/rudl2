use copper_core::{Bit, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::{emit, HardwareExecutor, HardwareTest};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[derive(Clone, Copy, Debug)]
enum State {
    S0, // no match
    S1, // saw '1'
    S2, // saw '10'
}

/// Mealy machine: detects "101". Output is 1 on the cycle the last '1' arrives.
#[hardware(function_typed)]
async fn mealy_101(
    clk: Clock<MainClk>,
    in_bit: Arc<Mutex<Bit>>,
) -> Bit {
    let mut state = State::S0;

    loop {
        let input = *in_bit.lock().unwrap();

        let output = match (state, input.0) {
            (State::S2, Logic::One) => Bit::ONE,
            (_, Logic::X) => Bit::X,
            _ => Bit::ZERO,
        };
        emit!(output);

        clk.tick().await;

        state = match (state, input.0) {
            (State::S0, Logic::One)  => State::S1,
            (State::S0, Logic::Zero) => State::S0,
            (State::S1, Logic::Zero) => State::S2,
            (State::S1, Logic::One)  => State::S1,
            (State::S2, Logic::One)  => State::S1,
            (State::S2, Logic::Zero) => State::S0,
            (_, Logic::X) => state,
        };
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let in_bit = Arc::new(Mutex::new(Bit::ZERO));
    let out_bit = exec.spawn_function_typed(
        Bit::ZERO,
        mealy_101(clk.clone(), Arc::clone(&in_bit)),
    );

    // Input stream — detects "101" at positions 3 and 6 (0-indexed)
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

    let mut test = HardwareTest::new("mealy")
        .with_verilog("verilog/mealy.v")
        .with_waveform("waveforms/mealy.vcd");

    println!("=== Mealy 101 Detector ===");
    for &bit in inputs.iter() {
        *in_bit.lock().unwrap() = Bit(bit);
        exec.tick_clock(&mut clk);
        let out = *out_bit.lock().unwrap();
        println!("cycle {} in={:?} out={:?}", clk.cycle(), bit, out.0);

        test.record_cycle(
            clk.cycle() as usize,
            &[("in_bit", &[bit])],
            &[("out_bit", &[out.0])],
        );
    }

    test.finish().assert_passed();
}
