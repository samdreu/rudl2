#[hardware(function_typed)]
async fn mealy_101(clk: Clock<MainClk>, in_bit: Arc<Mutex<Bit>>) -> Bit {
    let mut state = State::S0;  // state register — inferred from async structure
    loop {
        let input = *in_bit.lock().unwrap();
        let output = match (state, input.0) {
            (State::S2, Logic::One) => Bit::ONE,
            _                       => Bit::ZERO,
        };
        emit!(output);
        clk.tick().await;       // clock edge
        state = match (state, input.0) {
            (State::S0, Logic::One)  => State::S1,
            (State::S0, Logic::Zero) => State::S0,
            (State::S1, Logic::Zero) => State::S2,
            (State::S1, Logic::One)  => State::S1,
            (State::S2, Logic::One)  => State::S1,
            (State::S2, Logic::Zero) => State::S0,
            _                        => state,
        };
    }
}


match sel {
    Logic::One  => a,
    Logic::Zero => b,
    Logic::X    => Bit::X,  // compiler forces this arm
}
