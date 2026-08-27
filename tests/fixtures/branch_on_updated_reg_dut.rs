// A register ASSIGNED and then BRANCHED ON in the same segment.
//
// `c` is a register (it lives across the tick). The simulator runs the segment
// with Rust's own sequencing, so `if c == 3` reads the value `c = c + 1` just
// produced. A register update, though, becomes a non-blocking assignment at the
// edge — so an `if` emitted inside `always_ff` that reads `c` unforwarded reads
// the PRE-edge value and fires a cycle late.
//
// `n` changes every cycle, so a one-cycle-late guard captures a different byte
// rather than the same one at a different time.
#[hardware(sequential)]
async fn branch_on_updated_reg(
    clk: Clock<MainClk>,
    n: In<Bits<8>, MainClk>,
    o: RegOut<Bits<8>, MainClk>,
) {
    let mut c: Bits<8> = Bits::zero();
    loop {
        c = c + Bits::from_u8(1);
        if c == Bits::from_u8(3) {
            o.write(n.read());
            c = Bits::zero();
        }
        clk.tick().await;
    }
}

// The same claim with a BIT-SELECT guard. Forwarding substitutes the update
// into the condition, so the `Index` lands on a compound expression —
// `(c + 8'd1)[0]` — which SV forbids (a select cannot apply to a parenthesized
// expression). Until 2026-08-27 the emitter rendered exactly that, so this
// shape transpiled to SV that did not parse; `emit.rs` now emits the
// width-cast form `1'((c + 8'd1))` for a select over a non-identifier base.
// This module is what would catch that coming back, and pins that the cast
// form means the same bit as the simulator's `c[0]`.
#[hardware(sequential)]
async fn branch_on_updated_reg_bit(
    clk: Clock<MainClk>,
    n: In<Bits<8>, MainClk>,
    o: RegOut<Bits<8>, MainClk>,
) {
    let mut c: Bits<8> = Bits::zero();
    loop {
        c = c + Bits::from_u8(1);
        if c[0] == Logic::One {
            o.write(n.read());
        }
        clk.tick().await;
    }
}
