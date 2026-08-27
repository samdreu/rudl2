// Signedness — the sharpest case for running both sides rather than reading the
// SystemVerilog and nodding.
//
// HISTORY: `ExprType::Cast` used to be stripped in `chir_lower` and `signed`
// was never emitted, so `as i32` simply disappeared — the two `via_cast`
// modules below transpiled AND passed Verilator's `-Wall` cleanly while
// computing the wrong number, and only the differential sweep separated them
// from their working twins. FIXED 2026-08-27: `as i*` lowers to
// `CHIRExpr::SignCast` and propagates (`chir_lower::signed_binop`), so these
// now emit `$signed(a) < $signed(b)` and `$signed(instr) >>> 20` and SWEEP
// GREEN — their build.rs SKIP entries are deleted, which is the claim ledger
// working as designed. The `via_bias`/`via_mask` twins stay as the
// spelled-out controls.

/// `(a as i32) < (b as i32)` — a SIGNED compare (`$signed(a) < $signed(b)`).
/// This is RISC-V's SLT / BLT / BGE. Was an unsigned compare before the
/// SignCast fix; sweeps green now.
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

/// `as i32 >> 20` — an ARITHMETIC shift (`$signed(instr) >>> 20`), matching
/// Rust. This is RISC-V's I-type immediate. Was a logical shift (sign extension
/// silently became zero extension) before the SignCast fix; sweeps green now.
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

/// Propagation coverage (added with the 2026-08-27 fix): arithmetic on a signed
/// value BEFORE the compare. Two's complement makes the subtraction
/// bit-identical, but the comparison must still see signed operands — the
/// `SignCast` wrapper travels through `wrapping_sub` onto the result
/// (`chir_lower::signed_binop`), so this emits
/// `($signed((a - 1)) < $signed(b))`, not an unsigned compare of the difference.
#[hardware(sequential)]
pub async fn signed_lt_after_sub(
    clk: Clock<MainClk>,
    a: In<Bits<32>, MainClk>,
    b: In<Bits<32>, MainClk>,
    o: Out<Logic, MainClk>,
) {
    loop {
        clk.tick().await;
        let x: i32 = (a.read().as_u32() as i32).wrapping_sub(1);
        if x < (b.read().as_u32() as i32) {
            o.write(Logic::One);
        } else {
            o.write(Logic::Zero);
        }
    }
}
