//! File-scope `const` items → SystemVerilog `localparam`s.
//!
//! A `const WIDTH: usize = 8;` next to a `#[hardware]` module is visible to that
//! module's body and port types, but it is not reachable from the `ItemFn`, and
//! until this landed the transpiler reported `undefined variable 'WIDTH'` —
//! which is why `bsg_encode_one_hot` and `bsg_counter_up_down` had no
//! sim ≡ transpiled coverage at all.
//!
//! Two properties matter as much as "it works", and each has a test here:
//!
//! * an **unused** const must NOT be emitted — Verilator's `UNUSEDPARAM` makes a
//!   stray `localparam` a hard error under `-Wall`, so the emitted set has to be
//!   exact, not merely a superset;
//! * a const the transpiler cannot express must still be **refused**, with the
//!   reason attached — silently dropping it would turn a const into an undefined
//!   name at the use site.

fn transpile(src: &str) -> Result<String, String> {
    copper_codegen::transpile_source(src, None, &copper_codegen::EmitConfig::default())
        .map_err(|e| e.to_string())
}

/// The shape that motivated the work: a const in a **port width** and again as a
/// loop bound. The port width is why the declaration goes in the parameter port
/// list rather than the module body — a body declaration is not in scope for the
/// port list.
#[test]
fn a_const_used_in_a_port_width_and_a_loop_bound_becomes_a_localparam() {
    let src = r#"
const WIDTH: usize = 8;

#[hardware(combinational)]
fn enc(i: In<Bits<WIDTH>, ()>, v_o: Out<Logic, ()>) {
    let mut valid = Logic::Zero;
    for k in 0..WIDTH {
        if i.read()[k] == Logic::One {
            valid = Logic::One;
        }
    }
    v_o.write(valid);
}
"#;
    let sv = transpile(src).expect("a file-scope const must resolve");
    assert!(
        sv.contains("localparam int WIDTH = 8"),
        "expected a localparam declaration, got:\n{sv}"
    );
    // Declared in the parameter port list, ahead of the ports that use it.
    let lp = sv.find("localparam int WIDTH").expect("localparam present");
    let port = sv.find("[WIDTH-1:0]").expect("port width references WIDTH");
    assert!(lp < port, "the localparam must precede the port list, got:\n{sv}");
    assert!(sv.contains("k < WIDTH"), "the loop bound must stay symbolic, got:\n{sv}");
}

/// `localparam`, never `parameter`. A Rust `const` is a fixed value; emitting it
/// as an overridable parameter would let a synthesized module be elaborated at a
/// width no Copper simulation ever ran.
#[test]
fn a_const_is_not_emitted_as_an_overridable_parameter() {
    let src = r#"
const WIDTH: usize = 4;

#[hardware(combinational)]
fn passthru(i: In<Bits<WIDTH>, ()>, o: Out<Bits<WIDTH>, ()>) {
    o.write(i.read());
}
"#;
    let sv = transpile(src).expect("transpiles");
    assert!(sv.contains("localparam int WIDTH = 4"), "got:\n{sv}");
    assert!(
        !sv.contains("parameter int WIDTH"),
        "a const must not become an overridable parameter, got:\n{sv}"
    );
}

/// An unused const must not reach the output. This is not tidiness: Verilator
/// reports `UNUSEDPARAM` on an unreferenced `localparam` and `-Wall` turns it
/// into an error, so over-emitting would break every module that happens to sit
/// in a file with a testbench constant.
#[test]
fn an_unused_const_is_not_emitted() {
    let src = r#"
const WIDTH: usize = 8;
const CYCLES: usize = 200;

#[hardware(combinational)]
fn passthru(i: In<Bits<WIDTH>, ()>, o: Out<Bits<WIDTH>, ()>) {
    o.write(i.read());
}
"#;
    let sv = transpile(src).expect("transpiles");
    assert!(sv.contains("localparam int WIDTH = 8"), "got:\n{sv}");
    assert!(
        !sv.contains("CYCLES"),
        "a const the module never references must not be emitted, got:\n{sv}"
    );
}

/// The initializer is carried through as an expression, not evaluated, so the
/// derivation stays readable — and the const it depends on is pulled in with it
/// even though the body never names it directly, in dependency order.
#[test]
fn a_const_defined_from_another_const_keeps_its_expression_and_its_dependency() {
    let src = r#"
const PTR_W: usize = 3;
const MOD: usize = 1 << PTR_W;

#[hardware(combinational)]
fn wrap(i: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(i.read() % Bits::from_usize(MOD));
}
"#;
    let sv = transpile(src).expect("transpiles");
    assert!(sv.contains("localparam int MOD = 1 << PTR_W"), "got:\n{sv}");
    assert!(
        sv.contains("localparam int PTR_W = 3"),
        "a dependency of a used const must be emitted too, got:\n{sv}"
    );
    let ptr_w = sv.find("localparam int PTR_W").expect("PTR_W present");
    let mod_lp = sv.find("localparam int MOD").expect("MOD present");
    assert!(
        ptr_w < mod_lp,
        "SystemVerilog resolves the parameter list left to right, so PTR_W must \
         come first, got:\n{sv}"
    );
}

/// A `const fn` call is evaluated by rustc and nothing of it survives into the
/// emitted module, so the const has no SystemVerilog form. Referencing it is an
/// error — with the reason attached, so it reads as more than a typo. `mux.rs`
/// is the real instance (`safe_clog2(ELS_P)`).
#[test]
fn a_const_the_transpiler_cannot_express_is_refused_with_the_reason() {
    let src = r#"
const fn clog2(n: usize) -> usize { if n <= 1 { 0 } else { 1 + clog2(n / 2) } }
const ADDR_W: usize = clog2(8);

#[hardware(combinational)]
fn enc(i: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(i.read() + Bits::from_usize(ADDR_W));
}
"#;
    let err = transpile(src).expect_err("a const fn initializer has no SV form");
    assert!(err.contains("ADDR_W"), "the error must name the const, got: {err}");
    assert!(
        err.contains("const fn") || err.contains("cannot express"),
        "the error must explain why the const was skipped, got: {err}"
    );
}

/// A non-integer const is not a width or a bound; it is skipped for a different
/// reason, and the diagnostic says which.
#[test]
fn a_non_integer_const_is_refused_with_its_own_reason() {
    let src = r#"
const ENABLED: bool = true;

#[hardware(combinational)]
fn g(i: In<Logic, ()>, o: Out<Logic, ()>) {
    o.write(if ENABLED { i.read() } else { Logic::Zero });
}
"#;
    let err = transpile(src).expect_err("a bool const has no localparam form");
    assert!(err.contains("ENABLED"), "the error must name the const, got: {err}");
    assert!(
        err.contains("not an integer constant"),
        "the error must give the non-integer reason, got: {err}"
    );
}

/// Whole-identifier matching: a const must not be dragged in because its name is
/// a substring of another identifier. `WIDTH` appearing only inside `WIDTH_P`
/// would otherwise emit an unreferenced `localparam` — an `UNUSEDPARAM` error.
#[test]
fn a_const_whose_name_is_a_substring_of_another_is_not_pulled_in() {
    let src = r#"
const WIDTH: usize = 8;
const WIDTH_P: usize = 4;

#[hardware(combinational)]
fn passthru(i: In<Bits<WIDTH_P>, ()>, o: Out<Bits<WIDTH_P>, ()>) {
    o.write(i.read());
}
"#;
    let sv = transpile(src).expect("transpiles");
    assert!(sv.contains("localparam int WIDTH_P = 4"), "got:\n{sv}");
    assert!(
        !sv.contains("localparam int WIDTH ="),
        "`WIDTH` is only a substring of `WIDTH_P` here and must not be emitted, got:\n{sv}"
    );
}
