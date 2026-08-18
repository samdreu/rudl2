// Single source of truth for the six-state FSM equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// A Moore FSM with SIX enum states (past the 4-state traffic-light) — a ring that
// advances one step per cycle when `en` is high and holds otherwise. The state
// index is decoded to three `Logic` outputs (its binary bits) via a per-arm
// statement `match`, the same combinational-Moore shape the traffic light uses, so
// the transpiler emits an `always_comb` decode of the state register. Exercises
// enum-state encoding and a wider `match` at scale plus a conditional state update.
enum St {
    S0,
    S1,
    S2,
    S3,
    S4,
    S5,
}

#[hardware(sequential)]
async fn seq6(
    clk: Clock<MainClk>,
    en: In<Logic, MainClk>,
    b0: Out<Logic, MainClk>,
    b1: Out<Logic, MainClk>,
    b2: Out<Logic, MainClk>,
) {
    let mut st = St::S0;
    loop {
        // Moore decode: b2 b1 b0 = binary of the state index (statement-position
        // match writing Logic constants — combinational, not registered).
        match st {
            St::S0 => {
                b0.write(Logic::Zero);
                b1.write(Logic::Zero);
                b2.write(Logic::Zero);
            }
            St::S1 => {
                b0.write(Logic::One);
                b1.write(Logic::Zero);
                b2.write(Logic::Zero);
            }
            St::S2 => {
                b0.write(Logic::Zero);
                b1.write(Logic::One);
                b2.write(Logic::Zero);
            }
            St::S3 => {
                b0.write(Logic::One);
                b1.write(Logic::One);
                b2.write(Logic::Zero);
            }
            St::S4 => {
                b0.write(Logic::Zero);
                b1.write(Logic::Zero);
                b2.write(Logic::One);
            }
            St::S5 => {
                b0.write(Logic::One);
                b1.write(Logic::Zero);
                b2.write(Logic::One);
            }
        }

        clk.tick().await;

        st = match (st, en.read()) {
            (St::S0, Logic::One) => St::S1,
            (St::S1, Logic::One) => St::S2,
            (St::S2, Logic::One) => St::S3,
            (St::S3, Logic::One) => St::S4,
            (St::S4, Logic::One) => St::S5,
            (St::S5, Logic::One) => St::S0,
            (s, _) => s,
        };
    }
}
