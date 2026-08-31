// Single source of truth for the det_010 equivalence test.
// A 4-state Moore "010" detector, single-tick, with the output written AFTER the
// tick (a post-tick / trailing-segment Moore output) — the case that was silently
// dropped before the trailing port-drive fix. Also exercises `matches!`.
enum State { A, B, C, D }

#[hardware(sequential)]
async fn det_010(
    clk: Clock<MainClk>,
    rstn: In<Logic, MainClk>,
    in_i: In<Logic, MainClk>,
    out_o: Out<Logic, MainClk>,
) {
    let mut state = State::A;
    loop {
        if rstn.read() == Logic::Zero {
            state = State::A;
        } else {
            state = match (state, in_i.read()) {
                (State::A, Logic::Zero) => State::B,
                (State::B, Logic::One) => State::C,
                (State::B, Logic::Zero) => State::B,
                (State::C, Logic::Zero) => State::D,
                (State::D, Logic::Zero) => State::B,
                _ => State::A,
            };
        }
        clk.tick().await;

        if matches!(state, State::D) {
            out_o.write(Logic::One);
        } else {
            out_o.write(Logic::Zero);
        }
    }
}
