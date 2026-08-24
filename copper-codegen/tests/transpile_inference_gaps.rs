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
