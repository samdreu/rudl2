use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::{emit, HardwareExecutor};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[derive(Clone, Copy, Debug)]
enum State {
    START,
    ZERO,
    WAIT,
    S1,
    S2,
    S3,
}

// recognize the pattern 0010 in a serial stream
#[hardware(function_typed)]
async fn pattern_match(
    clk: Clock<MainClk>,
    rst: Arc<Mutex<Logic>>,
    in_bit: Arc<Mutex<Logic>>,
) -> Logic {
    let mut state = State::START;

    loop {
        let found;
        let rst_val = *rst.lock().unwrap();
        let input = *in_bit.lock().unwrap();
        if rst_val == Logic::One {
            state = State::START;
            found = Logic::Zero;
        } else {
            (state, found) = match (state, input) {
                (State::START, Logic::Zero) => (State::ZERO, Logic::Zero),
                (State::START, Logic::One) => (State::START, Logic::Zero),
                (State::ZERO, Logic::Zero) => (State::WAIT, Logic::Zero),
                (State::ZERO, Logic::One) => (State::START, Logic::Zero),
                (State::WAIT, Logic::Zero) => (State::S1, Logic::Zero),
                (State::WAIT, Logic::One) => (State::START, Logic::Zero),
                (State::S1, Logic::Zero) => (State::S2, Logic::Zero),
                (State::S1, Logic::One) => (State::START, Logic::Zero),
                (State::S2, Logic::One) => (State::S3, Logic::Zero),
                (State::S2, Logic::Zero) => (State::S1, Logic::Zero),
                (State::S3, Logic::Zero) => (State::WAIT, Logic::One),
                (State::S3, Logic::One) => (State::START, Logic::Zero),
                (_, Logic::X) => (state, Logic::X),
            };
        }
        emit!(found);

        clk.tick().await;
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let rst = Arc::new(Mutex::new(Logic::One));
    let in_bit = Arc::new(Mutex::new(Logic::Zero));
    let out_bit = exec.spawn_function_typed(
        Logic::Zero,
        pattern_match(clk.clone(), Arc::clone(&rst), Arc::clone(&in_bit)),
    );

    // Test sequence: 00010010, should detect pattern at the last bit
    let test_sequence = vec![
        Logic::Zero,
        Logic::Zero,
        Logic::Zero,
        Logic::One,
        Logic::Zero,
        Logic::Zero,
        Logic::One,
        Logic::Zero,
    ];

    // Release reset
    *rst.lock().unwrap() = Logic::Zero;

    for (i, bit) in test_sequence.into_iter().enumerate() {
        *in_bit.lock().unwrap() = bit;
        exec.tick_clock(&mut clk);
        let out = *out_bit.lock().unwrap();
        println!("cycle {}: input {:?}, output {:?}", i + 1, bit, out);
    }
}
