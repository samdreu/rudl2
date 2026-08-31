// Combinational temporaries: a wire that is reassigned, and a register that holds.
//
// A tuple is not a hardware type, so a branch that wants to produce several values
// has to produce them some other way. The obvious rewrite — `let mut`, assigned
// under an `if` — USED TO BE WRONG in a way that transpiled: every `Assign` became
// a register update whether or not the target was a register, so the signal got two
// drivers (an `always_comb` defining it and an `always_ff` holding it) and held last
// cycle's value where the simulator re-initialises it.
//
// FIXED 2026-08-26. `extract_updates_from_stmts` now emits a register update only
// for a name the register set actually contains, and the sequential path collects
// its segment's wire types so a reassigned wire lowers as another blocking assign.
// All three modules here sweep. They stay as the pin: this file is what named the
// bug, via `register_reconciliation.rs`, and what would catch it coming back.

/// `let mut` plus assignment inside branches — a combinational temporary. Lowers to
/// default-then-override in one `always_comb`, which is the idiom the transpiler
/// already used for its own memory control nets. This is the module that was broken
/// before the fix above; keep it, because it is the shape that regressing would
/// break first.
#[hardware(sequential)]
pub async fn held_temp_trap(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    sel: In<Logic, MainClk>,
    hi: Out<Bits<8>, MainClk>,
    lo: Out<Bits<8>, MainClk>,
) {
    loop {
        clk.tick().await;
        let mut u: Bits<8> = Bits::zero();
        let mut v: Bits<8> = Bits::zero();
        if sel.read() == Logic::One {
            u = a.read();
        } else if a.read() == Bits::zero() {
            v = Bits::<8>::from_u8(0xFF);
        }
        hi.write(u);
        lo.write(v);
    }
}

/// The working spelling: one TOTAL if-chain expression per value. Every path
/// assigns, so each stays combinational and single-driver.
#[hardware(sequential)]
pub async fn total_expr_temp(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    sel: In<Logic, MainClk>,
    hi: Out<Bits<8>, MainClk>,
    lo: Out<Bits<8>, MainClk>,
) {
    loop {
        clk.tick().await;
        let u: Bits<8> = if sel.read() == Logic::One {
            a.read()
        } else {
            Bits::zero()
        };
        let v: Bits<8> = if sel.read() == Logic::One {
            Bits::zero()
        } else if a.read() == Bits::zero() {
            Bits::<8>::from_u8(0xFF)
        } else {
            Bits::zero()
        };
        hi.write(u);
        lo.write(v);
    }
}

/// A `let mut` that IS meant to be a register — declared OUTSIDE the loop, so its
/// value is live across the tick. Emits an enabled flip-flop that holds. The pair
/// with `held_temp_trap` is the whole point: the two are syntactically identical at
/// the assignment, and only liveness across a tick separates them. A fix that made
/// either one look like the other would break this.
#[hardware(sequential)]
pub async fn enabled_register(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    en: In<Logic, MainClk>,
    q: Out<Bits<8>, MainClk>,
) {
    let mut held: Bits<8> = Bits::zero();
    loop {
        clk.tick().await;
        if en.read() == Logic::One {
            held = a.read();
        }
        q.write(held);
    }
}
