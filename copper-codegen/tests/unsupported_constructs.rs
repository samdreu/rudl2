//! Tracked regressions for constructs the transpiler does NOT support today
//! (P2, `TODO` TESTING plan). Distinct from `transpile_inference_gaps.rs` (which
//! pins bit-width *inference* gaps): these are constructs the lowering explicitly
//! rejects — an unsupported operator, method, or port type.
//!
//! Each test PINS the current "unsupported" boundary with the exact diagnostic, so
//! the limitation stays visible and a future contributor who adds support gets a
//! loud `expect_err` failure — the signal to promote the construct to a positive
//! equivalence test rather than relax the assertion.
//!
//! Discovered empirically via the `copper-transpile` CLI while filling the P2
//! construct matrix (see `TODO` TRANSPILATION). Supported neighbours that DO
//! transpile — and now have equivalence tests — are wide datapaths, `*`, `-`
//! (two's-complement negate), and `<<`/`>>`.

fn transpile(src: &str) -> Result<String, String> {
    copper_codegen::transpile_source(src, None, &copper_codegen::EmitConfig::default())
        .map_err(|e| e.to_string())
}

/// Array-typed ports (`In<[Bits<W>; ELS]>`) — the `mux` family. The example only
/// ever checks against a hand-written `mux.sv`; the transpiler cannot lower the
/// array port type, so there is no sim≡transpiled coverage for it yet.
const ARRAY_PORT_DUT: &str = r#"
#[hardware(combinational)]
fn mux4(data_i: In<[Bits<8>; 4], ()>, sel_i: In<Bits<2>, ()>, data_o: Out<Bits<8>, ()>) {
    data_o.write(data_i.read()[sel_i.read().as_u128() as usize]);
}
"#;

/// The division operator `/`. NOTE the asymmetry discovered while probing: `/` is
/// rejected, but `%` (remainder) transpiles fine to `(a % b)`. So this pins `/`
/// only — `%` is a supported construct (a Verilator equivalence for it would need
/// to guard against a zero divisor).
const DIV_DUT: &str = r#"
#[hardware(combinational)]
fn div8(a: In<Bits<8>, ()>, b: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(a.read() / b.read());
}
"#;

/// `arithmetic_shift_right` — the signed (sign-filling) shift. The simulator has
/// it (`Bits::arithmetic_shift_right`), but it is not in the transpiler's method
/// surface, so a signed-shift design has no sim≡transpiled coverage.
const ASR_DUT: &str = r#"
#[hardware(combinational)]
fn asr8(a: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(a.read().arithmetic_shift_right(2));
}
"#;

#[test]
fn array_typed_port_is_unsupported() {
    let err = transpile(ARRAY_PORT_DUT).expect_err(
        "NOW SUPPORTED: array-typed ports transpile. Give the mux family a real \
         sim≡transpiled equivalence test (it only had a hand-written mux.sv).",
    );
    assert!(
        err.contains("cannot resolve type") && err.contains("hardware type"),
        "reproduced a *different* transpile error than the tracked array-port gap: {err}"
    );
}

#[test]
fn division_operator_is_unsupported() {
    let err = transpile(DIV_DUT).expect_err("NOW SUPPORTED: `/` transpiles — add an equivalence test.");
    assert!(
        err.contains("'/'") && err.contains("not supported"),
        "reproduced a *different* transpile error than the tracked `/` gap: {err}"
    );
}

/// Guards the `/`-vs-`%` asymmetry: `%` really does transpile. If this ever starts
/// failing, `%` support regressed and the comment on `DIV_DUT` is stale.
#[test]
fn remainder_operator_is_supported() {
    let sv = transpile(
        r#"
        #[hardware(combinational)]
        fn rem8(a: In<Bits<8>, ()>, b: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
            o.write(a.read() % b.read());
        }
        "#,
    )
    .expect("`%` should transpile (it is supported, unlike `/`)");
    assert!(sv.contains("%"), "expected a `%` in the emitted SV, got:\n{sv}");
}

#[test]
fn arithmetic_shift_right_is_unsupported() {
    let err = transpile(ASR_DUT).expect_err(
        "NOW SUPPORTED: `arithmetic_shift_right` transpiles — add a signed-shift \
         equivalence test.",
    );
    assert!(
        err.contains("arithmetic_shift_right") && err.contains("not supported"),
        "reproduced a *different* transpile error than the tracked asr gap: {err}"
    );
}
