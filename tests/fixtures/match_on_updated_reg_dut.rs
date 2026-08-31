// The `match` half of the same seam: a register assigned — here through the two
// arms of an `if`, so the forwarded value is a merged mux — and then used as a
// `match` scrutinee in the SAME segment.
//
// `s` toggles, so the simulator writes `o` on every EVEN cycle. A `case` emitted
// inside `always_ff` on the unforwarded `s` sees the pre-edge value and writes on
// the ODD ones instead; `n` carries a different byte each cycle, so the two
// traces cannot be reconciled by shifting the stimulus.
#[hardware(sequential)]
async fn match_on_updated_reg(
    clk: Clock<MainClk>,
    n: In<Bits<8>, MainClk>,
    o: RegOut<Bits<8>, MainClk>,
) {
    let mut s: Logic = Logic::Zero;
    loop {
        if s == Logic::Zero {
            s = Logic::One;
        } else {
            s = Logic::Zero;
        }
        match s {
            Logic::One => {
                o.write(n.read());
            }
            _ => {}
        }
        clk.tick().await;
    }
}
