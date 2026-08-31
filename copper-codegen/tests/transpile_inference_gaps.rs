//! Bit-width inference: both gaps CLOSED.
//! (P0, `TODO` TESTING plan; see `TODO` TRANSPILATION section.)
//!
//! `transpile_source` used to reject two constructs the macro-simulator accepts,
//! both with "cannot infer bit width; add an explicit type annotation":
//!   1. `[Logic::Zero; N]` array locals (raw `Logic` arrays) — **CLOSED**, now
//!      asserted positively below, and behaviourally verified end-to-end by
//!      `tests/logic_array_pack_equivalence.rs` (sim ≡ transpiled SV ≡ an
//!      independent Rust golden, under Verilator).
//!   2. tuple-returning plain-Rust helper fns called from a hardware body
//!      (cause J-b) — **CLOSED 2026-08-27** by the tuple-destructuring `let`
//!      lowering: the helper call inlines (#7b) and the tuple projects per
//!      element. Asserted positively below; `ripple_carry_adder` itself now
//!      sweeps differentially (its build.rs SKIP entry is deleted).

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
fn tuple_returning_helper_transpiles() {
    let sv = transpile(TUPLE_HELPER_DUT)
        .expect("a tuple-returning helper fn must transpile (cause J-b closed)");
    // The helper inlines: `x` is the projected first element, `_y` is skipped.
    assert!(
        sv.contains("x = (i0 & i1);"),
        "the projected element should inline to the field expression, got:\n{sv}"
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

/// An integer local initialised from a **constant expression** — a module
/// parameter or file-scope const, with or without arithmetic.
///
/// This had no inferable width at all. `let mut k = WIDTH - 1;` asks both sides
/// of the subtraction for a type: `WIDTH` is not a signal, so it was not in the
/// symbol table, and `1` is a bare literal with no suffix — both came back
/// ambiguous and the module was rejected outright. Parameters are now seeded as
/// 32-bit, which is what they emit as (`parameter int` / `localparam int`), the
/// same width a `for`-loop variable already gets.
///
/// `examples/basejump/bsg_gray_to_binary.rs` is the real instance. Note it is
/// still not transpilable — with the width resolved it now reports its *actual*
/// remaining blocker, a `while` loop, which this says nothing about.
///
/// Scope: this asserts the construct LOWERS, not that any module using it lints.
/// A 32-bit local used purely as an index into a narrow vector draws a Verilator
/// `UNUSEDSIGNAL` on its upper bits — the same standing question the TODO
/// records, whether a bare integer local should take its width from its type or
/// from its uses. Fixing the ambiguity did not answer that.
#[test]
fn an_integer_local_from_a_const_expression_infers_a_width() {
    let src = r#"
const WIDTH: usize = 8;

#[hardware(combinational)]
fn m(i: In<Bits<WIDTH>, ()>, o: Out<Logic, ()>) {
    let k = WIDTH - 1;
    o.write(i.read()[k]);
}
"#;
    let sv = transpile(src).expect("a const-expression integer local must infer a width");
    assert!(
        sv.contains("logic [31:0] k;"),
        "a parameter-derived integer local is 32-bit, like an SV `int`, got:\n{sv}"
    );
    assert!(
        sv.contains("k = (WIDTH - 32'd1)"),
        "the initializer keeps the parameter symbolic, sized to 32 bits, got:\n{sv}"
    );
}

/// The same for a generic const parameter, which is the other way a module gets
/// one of these names.
#[test]
fn an_integer_local_from_a_generic_const_param_infers_a_width() {
    let src = r#"
#[hardware(combinational)]
fn m<const N: usize>(i: In<Bits<N>, ()>, o: Out<Logic, ()>) {
    let k = N - 1;
    o.write(i.read()[k]);
}
"#;
    let sv = transpile(src).expect("a generic const param must work the same way");
    assert!(sv.contains("logic [31:0] k;"), "got:\n{sv}");
    assert!(sv.contains("k = (N - 32'd1)"), "got:\n{sv}");
}
