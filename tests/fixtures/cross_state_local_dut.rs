// A local CAPTURED in one FSM state and read in another.
//
// The nested wait forces control extraction, which flattens the body into
// `match pc` arms — and an arm scopes its own locals, so `captured` used to be
// reported as an undefined variable at `o.write(captured)`. The identical body
// WITHOUT the wait (no extraction) has always transpiled, making `captured` a
// register; the two paths disagreed about the language's central rule, that a
// value live across an await becomes a register.
//
// `captured` must hold its value from the capture state through to the state
// that reads it, which is what the stimulus below checks: `d` changes while the
// module is between the two, so a `captured` that tracked `d` instead of holding
// would give a different answer.
#[hardware(sequential)]
async fn capture_after_wait(
    clk: Clock<MainClk>,
    go: In<Logic, MainClk>,
    d: In<Bits<8>, MainClk>,
    o: RegOut<Bits<8>, MainClk>,
) {
    loop {
        while go.read() == Logic::Zero {
            clk.tick().await;
        }
        let captured = d.read();
        clk.tick().await;
        o.write(captured);
        clk.tick().await;
    }
}
