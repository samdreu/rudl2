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
