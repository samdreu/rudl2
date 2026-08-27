// Signedness — the sharpest case for running both sides rather than reading the
// SystemVerilog and nodding.
//
// `ExprType::Cast` is stripped in `chir_lower` and the word `signed` is never
// emitted by the backend, so `as i32` simply disappears. The broken modules here
// transpile AND pass Verilator's `-Wall` cleanly: `assign o = (a >> 32'd20)` is
// perfectly good SystemVerilog that computes the wrong number. Only the
// differential sweep separates them from their working twins.

/// **BROKEN.** `(a as i32) < (b as i32)` emits an UNSIGNED compare. This is
/// RISC-V's SLT / BLT / BGE, and it is wrong for every negative operand.
#[hardware(sequential)]
pub async fn signed_lt_via_cast(
    clk: Clock<MainClk>,
    a: In<Bits<32>, MainClk>,
    b: In<Bits<32>, MainClk>,
    o: Out<Logic, MainClk>,
) {
    loop {
        clk.tick().await;
        if (a.read().as_u32() as i32) < (b.read().as_u32() as i32) {
            o.write(Logic::One);
        } else {
            o.write(Logic::Zero);
        }
    }
}

/// The working spelling: flipping the sign bit maps two's-complement order onto
/// unsigned order, so the unsigned compare the backend emits is the right one.
#[hardware(sequential)]
pub async fn signed_lt_via_bias(
    clk: Clock<MainClk>,
    a: In<Bits<32>, MainClk>,
    b: In<Bits<32>, MainClk>,
    o: Out<Logic, MainClk>,
) {
    loop {
        clk.tick().await;
        let bias: Bits<32> = Bits::<32>::from_u32(0x8000_0000);
        let sa: Bits<32> = a.read() ^ bias;
        let sb: Bits<32> = b.read() ^ bias;
        if sa.as_u32() < sb.as_u32() { o.write(Logic::One); } else { o.write(Logic::Zero); }
    }
}

/// **BROKEN.** `as i32 >> 20` is an ARITHMETIC shift in Rust and a LOGICAL shift
/// in the emitted SV, so sign extension silently becomes zero extension. This is
/// RISC-V's I-type immediate.
#[hardware(sequential)]
pub async fn sign_extend_via_cast(
    clk: Clock<MainClk>,
    instr: In<Bits<32>, MainClk>,
    o: Out<Bits<32>, MainClk>,
) {
    loop {
        clk.tick().await;
        o.write(Bits::<32>::from_u32((instr.read().as_u32() as i32 >> 20) as u32));
    }
}

/// The working spelling: extract the field, test its sign bit, OR in the fill.
#[hardware(sequential)]
pub async fn sign_extend_via_mask(
    clk: Clock<MainClk>,
    instr: In<Bits<32>, MainClk>,
    o: Out<Bits<32>, MainClk>,
) {
    loop {
        clk.tick().await;
        let w: Bits<32> = instr.read();
        let raw: Bits<32> = (w >> 20) & Bits::<32>::from_u32(0xFFF);
        let s: Bits<32> = (w >> 31) & Bits::<32>::from_u32(1);
        o.write(if s == Bits::<32>::from_u32(1) {
            raw | Bits::<32>::from_u32(0xFFFF_F000)
        } else {
            raw
        });
    }
}
