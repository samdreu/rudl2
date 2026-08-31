// Timing-reconciliation investigation fixture (EXECUTION_MODEL_RECONCILIATION.md).
//
// Two codings of the SAME intent — "every 2 cycles, sample `inp`, then drive it
// onto `out` one cycle later":
//
//   * `probe`     — the multi-tick async form (2 ticks/loop). This is the coding
//     whose sim-vs-transpiled timing is under dispute.
//   * `probe_fsm` — the explicit SINGLE-tick FSM (an explicit `phase` register).
//     Single-tick is the category where sim and transpiled Verilog PROVABLY
//     agree, so `probe_fsm` is a reference-quality, human-written witness of what
//     the hardware actually does (the same role `mac_fsm` plays for `mac_pipeline`).
//
// If `probe_fsm` (sim == verilog) picks one sampling schedule, that schedule IS
// the hardware-accurate answer, and whichever of {multi-tick sim, multi-tick
// transpiler} matches it is the correct execution model.

#[hardware(sequential)]
async fn probe(
    clk: Clock<MainClk>,
    inp: In<Bits<8>, MainClk>,
    out: Out<Bits<8>, MainClk>,
) {
    loop {
        let x = inp.read(); // x lives across .await → a register
        clk.tick().await; // edge A
        out.write(x);
        clk.tick().await; // edge B
    }
}

#[hardware(sequential, allow_pretick_alignment)]
async fn probe_fsm(
    clk: Clock<MainClk>,
    inp: In<Bits<8>, MainClk>,
    out: Out<Bits<8>, MainClk>,
) {
    let mut phase: u8 = 0;
    let mut x: Bits<8> = Bits::from_lit::<0>();
    loop {
        if phase == 0 {
            x = inp.read();
            phase = 1;
        } else {
            out.write(x.clone());
            phase = 0;
        }
        clk.tick().await;
    }
}
