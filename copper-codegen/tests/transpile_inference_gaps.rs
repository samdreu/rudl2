//! Tracked regressions for a KNOWN transpiler bit-width-inference GAP
//! (P0, `TODO` TESTING plan; see `TODO` TRANSPILATION section).
//!
//! `transpile_source` currently rejects two constructs that the macro-simulator
//! accepts, with "cannot infer bit width; add an explicit type annotation":
//!   1. `[Logic::Zero; N]` array locals (raw `Logic` arrays), and
//!   2. tuple-returning plain-Rust helper fns called from a hardware body.
//!
//! These tests PIN that current behavior so the gap stays visible and honest — the
//! equivalence tests for the affected designs (ripple_carry_adder, gray_to_binary)
//! use `Bits`-indexing rewrites to sidestep it, which could otherwise let the
//! limitation be silently forgotten.
//!
//! **When the gap is fixed** these `expect_err`s will fail loudly. That is the
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
fn logic_array_local_is_a_known_transpile_gap() {
    let err = transpile(LOGIC_ARRAY_DUT).expect_err(
        "KNOWN GAP now closed: `[Logic; N]` array locals transpile. Remove the \
         Bits-indexing workaround in the ripple/gray fixtures and make this a \
         positive equivalence test.",
    );
    assert!(
        err.contains("cannot infer bit width"),
        "reproduced a *different* transpile error than the tracked bit-width gap: {err}"
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
