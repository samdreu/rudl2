// WHERE an output is written, in a SINGLE-TICK loop.
//
// Found by the corpus sweep on `rv32i_cpu_transpilable`'s `program_counter`, on
// its first run, twice. Neither shape is caught by D1's guard, by
// `multi_phase_out_write`, or by Verilator — the emitted SystemVerilog is
// well-formed in every case. Only running both sides shows it.
//
// The distinguishing feature is that the loop crosses exactly ONE clock edge, so
// the trailing statements share the head's phase. Every existing member of the
// pre-tick alignment family is about a body with two or more edges.

/// **BROKEN.** A plain `Out` driven from a register, written BEFORE the register
/// commits. The simulator carries the value the register held when the statement
/// ran; the emitted `assign` reads the register itself, which has already moved on
/// — so the hardware leads by a cycle. D1's guard exempts this segment because an
/// `In` read precedes the write.
#[hardware(sequential)]
pub async fn out_from_reg_before_commit(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    q: Out<Bits<8>, MainClk>,
) {
    let mut acc: Bits<8> = Bits::zero();
    loop {
        clk.tick().await;
        let d: Bits<8> = step.read();
        q.write(acc);
        acc = acc + d;
    }
}

/// **BROKEN, and not fixed by `RegOut`** — the half worth knowing. In a
/// single-tick loop the trailing statements share the head's phase, so the
/// transpiler folds a trailing `RegOut` write into THIS edge while the simulator
/// commits it on the NEXT one. Same one-cycle lead, opposite cause.
#[hardware(sequential)]
pub async fn regout_trailing_single_tick(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    q: RegOut<Bits<8>, MainClk>,
) {
    let mut acc: Bits<8> = Bits::zero();
    loop {
        clk.tick().await;
        let d: Bits<8> = step.read();
        acc = acc + d;
        q.write(acc);
    }
}

/// The shape where the two agree: a plain `Out` written AFTER the commit. The
/// emitted continuous assignment tracks the register, and the simulator writes
/// that same register's post-commit value in the same cycle.
#[hardware(sequential)]
pub async fn out_from_reg_after_commit(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    q: Out<Bits<8>, MainClk>,
) {
    let mut acc: Bits<8> = Bits::zero();
    loop {
        clk.tick().await;
        let d: Bits<8> = step.read();
        acc = acc + d;
        q.write(acc);
    }
}
