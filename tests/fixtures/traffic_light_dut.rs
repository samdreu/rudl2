// Single source of truth for the traffic-light equivalence test.
// Exercises M2 class-B2: pattern bindings (`t`), arm guards (`if t < 1`), and
// partial wildcards inside a tuple pattern, plus tuple destructuring assignment.
enum Phase {
    Green,
    Yellow,
    Red,
    RedYellow,
}

#[hardware(sequential)]
async fn traffic_light(
    clk: Clock<MainClk>,
    request: In<Logic, MainClk>,
    red: Out<Logic, MainClk>,
    yellow: Out<Logic, MainClk>,
    green_out: Out<Logic, MainClk>,
) {
    let mut phase = Phase::Green;
    let mut timer: u8 = 0;

    loop {
        match phase {
            Phase::Green => {
                red.write(Logic::Zero);
                yellow.write(Logic::Zero);
                green_out.write(Logic::One);
            }
            Phase::Yellow => {
                red.write(Logic::Zero);
                yellow.write(Logic::One);
                green_out.write(Logic::Zero);
            }
            Phase::Red => {
                red.write(Logic::One);
                yellow.write(Logic::Zero);
                green_out.write(Logic::Zero);
            }
            Phase::RedYellow => {
                red.write(Logic::One);
                yellow.write(Logic::One);
                green_out.write(Logic::Zero);
            }
        }

        clk.tick().await;

        (phase, timer) = match (phase, timer, request.read()) {
            (Phase::Green, _, Logic::One) => (Phase::Yellow, 0),
            (Phase::Green, _, _) => (Phase::Green, 0),
            (Phase::Yellow, t, _) if t < 1 => (Phase::Yellow, t + 1),
            (Phase::Yellow, _, _) => (Phase::Red, 0),
            (Phase::Red, t, _) if t < 3 => (Phase::Red, t + 1),
            (Phase::Red, _, _) => (Phase::RedYellow, 0),
            (Phase::RedYellow, _, _) => (Phase::Green, 0),
        };
    }
}
