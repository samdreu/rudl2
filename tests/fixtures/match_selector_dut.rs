// What a `match` scrutinee may be, and what its case labels come out as.
//
// Cause D-a made a file-scope `const` work as an EXPRESSION (it lowers to a
// localparam). It does not work as a PATTERN, and a `match` on a `usize` emits
// 64-bit case labels. Both are invisible to "does it transpile"; the second is
// invisible to everything but Verilator.

const OP_ALPHA: u32 = 0x37;
const OP_BETA:  u32 = 0x6F;

/// FIXED 2026-08-27 (was: a `match` emitted 64-bit case labels — suffix-less
/// pattern literals now take the scrutinee's width, value-fit guarded).
/// Measured against a `usize`
/// scrutinee here and against a `u32` in `match_on_literals` below: the width comes
/// from the LITERAL, not from the scrutinee, so no integer type escapes it.
#[hardware(sequential)]
pub async fn match_on_usize(
    clk: Clock<MainClk>,
    sel: In<Bits<2>, MainClk>,
    a: In<Bits<16>, MainClk>,
    o: Out<Bits<16>, MainClk>,
) {
    let mut x1: Bits<16> = Bits::zero();
    let mut x2: Bits<16> = Bits::zero();
    loop {
        clk.tick().await;
        let s: usize = sel.read().as_usize();
        match s {
            1 => x1 = a.read(),
            2 => x2 = a.read(),
            _ => {}
        }
        let v: Bits<16> = match s {
            1 => x1,
            2 => x2,
            _ => Bits::zero(),
        };
        o.write(v);
    }
}

/// The working spelling: the same mux and demux as an if/else-if chain, where the
/// comparison takes its width from the selector.
#[hardware(sequential)]
pub async fn ifchain_on_usize(
    clk: Clock<MainClk>,
    sel: In<Bits<2>, MainClk>,
    a: In<Bits<16>, MainClk>,
    o: Out<Bits<16>, MainClk>,
) {
    let mut x1: Bits<16> = Bits::zero();
    let mut x2: Bits<16> = Bits::zero();
    loop {
        clk.tick().await;
        let s: usize = sel.read().as_usize();
        if s == 1 { x1 = a.read(); } else if s == 2 { x2 = a.read(); }
        let v: Bits<16> = if s == 1 { x1 } else if s == 2 { x2 } else { Bits::zero() };
        o.write(v);
    }
}

/// STILL REFUSED, now with an honest diagnostic (2026-08-27): a named `const`
/// as a match PATTERN is named as exactly that, pointing at the if-chain
/// spelling. Full support is a name-carrying pattern kind through the IRs — a
/// feature, tracked in the SKIP entry. Previously it read as an enum-variant
/// pattern and is refused — with a message about tuple patterns, which is not
/// what it is.
#[hardware(sequential)]
pub async fn match_on_const_pattern(
    clk: Clock<MainClk>,
    instr: In<Bits<32>, MainClk>,
    o: Out<Logic, MainClk>,
) {
    loop {
        clk.tick().await;
        let op: u32 = instr.read().as_u32() & 0x7F;
        let known: bool = match op {
            OP_ALPHA | OP_BETA => true,
            _ => false,
        };
        if known { o.write(Logic::One); } else { o.write(Logic::Zero); }
    }
}

/// The same decode with the consts in EXPRESSION position, which is where they do
/// lower. This is the shape `rv32i_cpu_transpilable` uses.
#[hardware(sequential)]
pub async fn ifchain_on_const_expr(
    clk: Clock<MainClk>,
    instr: In<Bits<32>, MainClk>,
    o: Out<Logic, MainClk>,
) {
    loop {
        clk.tick().await;
        let op: u32 = instr.read().as_u32() & 0x7F;
        let known: bool = op == OP_ALPHA || op == OP_BETA;
        if known { o.write(Logic::One); } else { o.write(Logic::Zero); }
    }
}

/// FIXED 2026-08-27 (and originally written as a control expected to pass —
/// which is how the claim was narrowed to "the literal decides"). A `match` on a
/// `u32` with literal patterns, which transpiles and which I expected to Verilate.
/// It emits `op == 64'd55`. That is what narrowed the claim above from "a `match`
/// on a `usize`" to "a `match` literal", and it is the reason this file exists: the
/// probe that only asked "does it transpile" had already called this one working.
#[hardware(sequential)]
pub async fn match_on_literals(
    clk: Clock<MainClk>,
    instr: In<Bits<32>, MainClk>,
    o: Out<Logic, MainClk>,
) {
    loop {
        clk.tick().await;
        let op: u32 = instr.read().as_u32() & 0x7F;
        let known: bool = match op {
            0x37 | 0x6F => true,
            _ => false,
        };
        if known { o.write(Logic::One); } else { o.write(Logic::Zero); }
    }
}
