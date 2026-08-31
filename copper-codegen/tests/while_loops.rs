//! `while` loops: one form is sugar, the other is refused.
//!
//! The two uses in the corpus were filed under a single "while loops
//! unsupported" cause because they print the same message. They are unrelated:
//!
//! * `while <cond> { … clk.tick().await; }` (uart/rx) is a data-dependent WAIT.
//!   It is exactly `loop { if !<cond> { break; } … }` — test first, tick last,
//!   which is the supported ordering — so it is rewritten before control
//!   extraction and lands on machinery already verified end to end.
//! * `while k > 0 { k -= 1; … }` (bsg_gray_to_binary, before it was rewritten)
//!   is COMBINATIONAL. To be hardware it must fully unroll, which needs a trip
//!   count known at compile time. `for` states that; `while` only implies it, so
//!   it stays refused — with a diagnostic that names `for` rather than the
//!   generic "use a loop with a tick" advice, which would be wrong here.
//!
//! `tests/while_wait_equivalence.rs` at the repo root carries the behavioural
//! half: the desugared form must simulate and Verilate identically to the
//! hand-written one.

fn transpile(src: &str) -> Result<String, String> {
    copper_codegen::transpile_source(src, None, &copper_codegen::EmitConfig::default())
        .map_err(|e| e.to_string())
}

const WAIT_WHILE: &str = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, go: In<Logic, MainClk>, o: RegOut<Bits<8>, MainClk>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        while go.read() == Logic::One {
            clk.tick().await;
        }
        n = n + Bits::from_lit::<1>();
        o.write(n);
        clk.tick().await;
    }
}
"#;

/// The hand-written spelling of the same wait, which has transpiled since
/// control extraction increment B.
const WAIT_LOOP: &str = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, go: In<Logic, MainClk>, o: RegOut<Bits<8>, MainClk>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        loop {
            if !(go.read() == Logic::One) { break; }
            clk.tick().await;
        }
        n = n + Bits::from_lit::<1>();
        o.write(n);
        clk.tick().await;
    }
}
"#;

#[test]
fn a_while_containing_a_tick_transpiles_as_a_repeating_wait() {
    let sv = transpile(WAIT_WHILE).expect("a tick-bearing `while` must transpile");
    assert!(
        sv.contains("case (pc)"),
        "a wait must flatten to a pc FSM, got:\n{sv}"
    );
}

/// The desugar must be exactly that — sugar. If the two spellings ever emit
/// different SystemVerilog, the rewrite has started meaning something.
#[test]
fn the_while_and_loop_spellings_emit_identical_verilog() {
    let from_while = transpile(WAIT_WHILE).expect("while form transpiles");
    let from_loop = transpile(WAIT_LOOP).expect("loop form transpiles");
    assert_eq!(
        from_while, from_loop,
        "`while c {{ …tick }}` and `loop {{ if !c {{ break }} …tick }}` are the same \
         program and must emit the same module"
    );
}

/// A `while` with no tick is combinational and must unroll, which `while` cannot
/// promise. Refused — and the diagnostic points at `for`, not at "add a tick",
/// which would be the wrong fix for a combinational module.
#[test]
fn a_combinational_while_is_refused_and_points_at_for() {
    let src = r#"
#[hardware(combinational)]
fn m(i: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    let g = i.read();
    let mut b = [Logic::Zero; 8];
    let mut k: usize = 7;
    while k > 0 { k -= 1; b[k] = g[k]; }
    o.write(Bits::from_slice(&b));
}
"#;
    let err = transpile(src).expect_err("a combinational while cannot be shown to terminate");
    assert!(
        err.contains("`for`"),
        "the diagnostic must name the construct that works, got: {err}"
    );
    assert!(
        err.contains("compile time"),
        "the diagnostic must say WHY a while cannot do this, got: {err}"
    );
    assert!(
        !err.contains("clk.tick().await; }`"),
        "must not advise adding a tick to a combinational module, got: {err}"
    );
}
