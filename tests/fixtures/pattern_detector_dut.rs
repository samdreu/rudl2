// Single source of truth for the pattern-detector equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// The enum is file-scope on purpose: it exercises the file-scope enum injection.
#[derive(PartialEq)]
enum State {
    IDLE = 0,
    S1 = 1,
    S11 = 2,
    S110 = 3,
    S1101 = 4,
    S11010 = 5,
    S110101 = 6,
}

#[hardware(sequential)]
async fn det_110101(
    clk: Clock<MainClk>,
    rstn: In<Logic, MainClk>,
    in_i: In<Logic, MainClk>,
    out_o: Out<Logic, MainClk>,
) {
    let mut state = State::IDLE;
    loop {
        if rstn.read() == Logic::Zero {
            state = State::IDLE;
        } else {
            state = match (state, in_i.read()) {
                (State::IDLE, Logic::One) => State::S1,
                (State::S1, Logic::One) => State::S11,
                (State::S11, Logic::Zero) => State::S110,
                (State::S110, Logic::One) => State::S1101,
                (State::S1101, Logic::Zero) => State::S11010,
                (State::S11010, Logic::One) => State::S110101,
                _ => State::IDLE,
            };
        }
        if state == State::S110101 {
            out_o.write(Logic::One);
        } else {
            out_o.write(Logic::Zero);
        }
        clk.tick().await;
    }
}
