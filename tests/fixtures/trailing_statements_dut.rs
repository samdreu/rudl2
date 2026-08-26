// Trailing combinational statements — statements after the LAST `clk.tick().await`
// of a multi-tick loop.
//
// The semantics, decided 2026-08-25 from `design_docs/SYNCHRONOUS_SEMANTICS.md`
// ("a clock cycle is a maximal tick-free region of an execution"): the trailing
// statements and the head's are ONE cycle, because falling off the end of the body
// and re-entering it costs no clock. Their combinational logic therefore belongs to
// phase 0, and the simulator runs them BEFORE the head's — the trailing statements in
// the post-edge settle of the tick that opens the cycle, the head's in the pre-edge
// settle of the tick that closes it.
//
// The multi-tick path used to refuse these outright, while the single-tick path
// hoisted them and the extracted path accepted them; the corpus sweep generates a
// differential case for each module here, so the decision is checked against the
// emitted SystemVerilog rather than asserted.

/// An output driven in the trailing segment, alongside one driven mid-loop. This is
/// the shape the multi-tick path refused.
///
/// `tail` is a `RegOut` because a plain `Out` driven from a REGISTER here is the
/// pre-tick hazard past the last tick, which `unprotected_trailing_out_write` rejects
/// — the two things are separate: the lowering can now express trailing statements,
/// and the rule still refuses the subset that diverges. What stays legal in the
/// trailing segment with a plain `Out` is a constant write (`uart/rx`'s
/// `rx_dv.write(Zero)`, verified) — see `trailing_constant` below.
#[hardware(sequential)]
async fn trailing_out(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    mid: Out<Bits<8>, MainClk>,
    tail: RegOut<Bits<8>, MainClk>,
) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = a.read();
        clk.tick().await;
        mid.write(r);
        clk.tick().await;
        tail.write(r);
    }
}

/// A plain local computed in the trailing segment and consumed by the HEAD. It lives
/// across no tick, so it is a wire and not a register — which makes it the one thing
/// that can observe the emission order within the merged cycle. If the head were
/// emitted first it would read a stale value.
#[hardware(sequential)]
async fn trailing_local_into_head(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    o: Out<Bits<8>, MainClk>,
) {
    let mut r: Bits<8> = Bits::zero();
    let mut w: Bits<8> = Bits::zero();
    loop {
        o.write(w + r);
        clk.tick().await;
        r = a.read();
        clk.tick().await;
        w = r + Bits::from_lit::<1>();
    }
}

/// A plain `Out` written with a CONSTANT in the trailing segment — legal, and the
/// real instance is `uart/rx`'s `rx_dv.write(Zero)`. The value is the same whichever
/// phase the write runs in, so the alignment question cannot be observed through it.
#[hardware(sequential)]
async fn trailing_constant(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    dv: Out<Logic, MainClk>,
    o: Out<Bits<8>, MainClk>,
) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = a.read();
        clk.tick().await;
        o.write(r);
        clk.tick().await;
        dv.write(Logic::One);
    }
}
