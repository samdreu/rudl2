// Struct latches, tuple destructuring, and block bindings — the pipelined-CPU
// shapes, landed 2026-08-27.
//
// Milestone 2 gave struct WIRES (`let p = P { … }` in-loop, per-field
// `<base>_<field>` nets). What the RV32I pipeline actually needs is the rest of
// the aggregate surface, and each module here is the minimal form of one shape:
//
//   - `struct_latch`         — a struct-valued pre-loop `let mut` (a pipeline
//                              latch) flattens to one REGISTER per field, with
//                              the register authority consulted for the whole
//                              name and ctor inits (`Self { … }` through
//                              `X::bubble()`) carried per field.
//   - `struct_latch_cond`    — a struct-valued `if`-else `let` lowers
//                              default-then-override: zero-defaulted field
//                              wires, then the conditional as a statement whose
//                              leaves assign fields — so arm-local `let`s
//                              survive (an expression projection would drop
//                              them). Whole-struct assign copies field-wise.
//   - `tuple_destructure`    — `let (a, b, …) = <if tree>` binds per element
//                              by the same transform; a struct-typed element
//                              recursively gets per-field wires.
//   - `block_binding_shadow` — `let x = { let t = …; tail }` twice, both inner
//                              `let`s named `t`. The inner binders are renamed
//                              `x_t`, so the second block does not silently
//                              read the first block's temporary. This module is
//                              the pin for that rename: without it, both
//                              outputs would compute from ONE `t`.
//   - `sra_method`           — `a.arithmetic_shift_right(n)` is the signed
//                              shift `$signed(a) >>> n`, the same lowering the
//                              `as i32` cast path gets (signedness_dut anchors
//                              that semantics; this pins the method spelling).

struct Latch {
    valid: bool,
    data: Bits<8>,
}

impl Latch {
    fn bubble() -> Self {
        Self { valid: false, data: Bits::zero() }
    }
}

/// A struct-valued pre-loop `let mut` — the IF/ID latch shape. One register per
/// field; head write publishes the committed value, the trailing segment loads.
#[hardware(sequential)]
pub async fn struct_latch(
    clk: Clock<MainClk>,
    x: In<Bits<8>, MainClk>,
    en: In<Logic, MainClk>,
    o: Out<Bits<8>, MainClk>,
) {
    let mut latch = Latch::bubble();
    loop {
        if latch.valid {
            o.write(latch.data);
        } else {
            o.write(Bits::zero());
        }
        let xv = x.read();
        let env = en.read();
        clk.tick().await;
        latch = Latch { valid: env == Logic::One, data: xv };
    }
}

/// A struct-valued conditional `let` with an arm-local `let`, then a
/// whole-struct assignment into the latch — the EX-stage shape.
#[hardware(sequential)]
pub async fn struct_latch_cond(
    clk: Clock<MainClk>,
    x: In<Bits<8>, MainClk>,
    sel: In<Logic, MainClk>,
    o: Out<Bits<8>, MainClk>,
) {
    let mut latch = Latch::bubble();
    loop {
        if latch.valid {
            o.write(latch.data);
        } else {
            o.write(Bits::zero());
        }
        let xv = x.read();
        let sv = sel.read();
        clk.tick().await;
        let next = if sv == Logic::One {
            let bumped = xv + Bits::<8>::from_lit::<1>();
            Latch { valid: true, data: bumped }
        } else {
            Latch::bubble()
        };
        latch = next;
    }
}

/// `let (st, flag, tgt) = <if tree>` — a struct element, a bool, and a value,
/// exactly the EX stage's `(new_ex_mem, flush, branch_target)` shape.
#[hardware(sequential)]
pub async fn tuple_destructure(
    clk: Clock<MainClk>,
    x: In<Bits<8>, MainClk>,
    sel: In<Logic, MainClk>,
    o: Out<Bits<8>, MainClk>,
    t: Out<Bits<8>, MainClk>,
) {
    let mut latch = Latch::bubble();
    let mut target: Bits<8> = Bits::zero();
    loop {
        if latch.valid {
            o.write(latch.data);
        } else {
            o.write(Bits::zero());
        }
        t.write(target);
        let xv = x.read();
        let sv = sel.read();
        clk.tick().await;
        let (st, flag, tgt) = if sv == Logic::One {
            let doubled = xv + xv;
            (Latch { valid: true, data: doubled }, true, xv + Bits::<8>::from_lit::<1>())
        } else {
            (Latch::bubble(), false, Bits::zero())
        };
        latch = st;
        if flag {
            target = tgt;
        } else {
            target = Bits::zero();
        }
    }
}

/// Two block bindings whose inner `let`s share a name — the forwarding-unit
/// shape. The rename pin: `hit` must stay per-block.
#[hardware(sequential)]
pub async fn block_binding_shadow(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    b: In<Bits<8>, MainClk>,
    o1: Out<Bits<8>, MainClk>,
    o2: Out<Bits<8>, MainClk>,
) {
    loop {
        clk.tick().await;
        let fwd_a = {
            let hit = a.read().as_u8() > b.read().as_u8();
            if hit { a.read() } else { b.read() }
        };
        let fwd_b = {
            let hit = b.read().as_u8() > a.read().as_u8();
            if hit { b.read() } else { a.read() }
        };
        o1.write(fwd_a);
        o2.write(fwd_b);
    }
}

/// `a.arithmetic_shift_right(n)` — the SRA method spelling.
#[hardware(sequential)]
pub async fn sra_method(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    o: Out<Bits<8>, MainClk>,
) {
    loop {
        clk.tick().await;
        let av = a.read();
        o.write(av.arithmetic_shift_right(3));
    }
}
