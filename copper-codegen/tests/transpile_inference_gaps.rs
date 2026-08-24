//! Bit-width inference: one gap CLOSED, one still open.
//! (P0, `TODO` TESTING plan; see `TODO` TRANSPILATION section.)
//!
//! `transpile_source` used to reject two constructs the macro-simulator accepts,
//! both with "cannot infer bit width; add an explicit type annotation":
//!   1. `[Logic::Zero; N]` array locals (raw `Logic` arrays) — **CLOSED**, now
//!      asserted positively below, and behaviourally verified end-to-end by
//!      `tests/logic_array_pack_equivalence.rs` (sim ≡ transpiled SV ≡ an
//!      independent Rust golden, under Verilator).
//!   2. tuple-returning plain-Rust helper fns called from a hardware body —
//!      **still open**, still pinned.
//!
//! The open one PINS current behavior so the gap stays visible and honest — the
//! equivalence test for the affected design (ripple_carry_adder) uses a
//! `Bits`-indexing rewrite to sidestep it, which could otherwise let the
//! limitation be silently forgotten.
//!
//! **When that gap is fixed** its `expect_err` will fail loudly. That is the
//! signal to delete the workaround and promote the natural form to a positive
//! equivalence test — do NOT just relax the assertion.

fn transpile(src: &str) -> Result<String, String> {
    copper_codegen::transpile_source(src, None, &copper_codegen::EmitConfig::default())
        .map_err(|e| e.to_string())
}

const LOGIC_ARRAY_DUT: &str = r#"
#[hardware(combinational)]
fn logic_array<const N: usize>(i: In<Bits<N>, ()>, o: Out<Bits<N>, ()>) {
    let x = i.read();
    let mut a = [Logic::Zero; N];
    for k in 0..N {
        a[k] = x[k];
    }
    o.write(Bits::from_slice(&a));
}
"#;

const TUPLE_HELPER_DUT: &str = r#"
fn pair(a: Logic, b: Logic) -> (Logic, Logic) {
    (a & b, a | b)
}

#[hardware(combinational)]
fn use_pair(i0: In<Logic, ()>, i1: In<Logic, ()>, o: Out<Logic, ()>) {
    let (x, _y) = pair(i0.read(), i1.read());
    o.write(x);
}
"#;

#[test]
fn logic_array_local_transpiles_as_a_packed_vector() {
    let sv = transpile(LOGIC_ARRAY_DUT)
        .expect("`[Logic::Zero; N]` array locals must transpile (gap closed)");

    // The array local is a packed vector of the repeat length — symbolic here,
    // so it must carry the module parameter rather than a guessed width.
    assert!(
        sv.contains("logic [N-1:0] a;"),
        "array local should be an N-bit packed vector, got:\n{sv}"
    );
    // Element k is bit k, so an indexed write stays an indexed write...
    assert!(
        sv.contains("a[k] = x[k];"),
        "indexed writes into the array should lower as bit-selects, got:\n{sv}"
    );
    // ...and `Bits::from_slice` moves no bits, so it lowers to identity.
    assert!(
        sv.contains("assign o = a;"),
        "from_slice should lower to identity, got:\n{sv}"
    );
}

#[test]
fn tuple_returning_helper_is_a_known_transpile_gap() {
    let err = transpile(TUPLE_HELPER_DUT).expect_err(
        "KNOWN GAP now closed: tuple-returning helper fns transpile. Promote the \
         natural full-adder form to a positive equivalence test.",
    );
    assert!(
        err.contains("cannot infer bit width"),
        "reproduced a *different* transpile error than the tracked bit-width gap: {err}"
    );
}

/// `usize` must mean ONE width, whichever way the source spells it.
///
/// It did not. `resolve_type` resolved `usize` to 32 bits — a deliberate choice
/// (commit 78d91f7, 2026-07-24: "matches SV `int` loop var, keeps index
/// arithmetic width-consistent"), which `Bits::from_usize` was written to mirror
/// — but the two LITERAL-SUFFIX tables still carried the first draft's 64, so
/// `let x = 0usize` and `let x: usize = 0` produced a 64-bit and a 32-bit signal
/// from the same Rust type.
///
/// That is worse than a cosmetic inconsistency: nothing stopped the two from
/// meeting in one expression, where the mismatch becomes a silent width
/// conversion rather than an error. Both spellings appear in the corpus —
/// `bsg_encode_one_hot` uses the suffix form, `bsg_counter_up_down` the
/// annotation.
#[test]
fn both_spellings_of_usize_give_the_same_width() {
    let src = r#"
#[hardware(combinational)]
fn m(i: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    let mut suffixed = 0usize;
    let mut annotated: usize = 0;
    if i.read()[0] == Logic::One {
        suffixed = 3;
        annotated = 4;
    }
    o.write(Bits::from_usize(suffixed) + Bits::from_usize(annotated));
}
"#;
    let sv = transpile(src).expect("transpiles");
    let widths: Vec<&str> = sv
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            let name = t.strip_prefix("logic [")?;
            let (range, rest) = name.split_once("] ")?;
            (rest.starts_with("suffixed") || rest.starts_with("annotated")).then_some(range)
        })
        .collect();
    assert_eq!(widths.len(), 2, "both locals must be declared, got:\n{sv}");
    assert_eq!(
        widths[0], widths[1],
        "`0usize` and `: usize = 0` are the same Rust type and must emit the \
         same width, got:\n{sv}"
    );
    assert_eq!(widths[0], "31:0", "usize is 32-bit throughout, got:\n{sv}");
}
