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

// ── Memory is not transpilable at all (P4) ───────────────────────────────────

/// `Memory` — the first-class memory construct — has **no transpiled path**.
///
/// This is a larger gap than the operator/port limitations above: it is not one
/// construct being rejected, it is an entire feature that exists only in the
/// simulator. `examples/memory/dual_port_ram.rs` is a shipped example with an
/// independent hand-written SV reference (`examples/memory/sv/dual_port_ram.sv`),
/// and its Copper source cannot be lowered.
///
/// **What this costs:** every memory guarantee is sim-only. `tests/memory_new.rs`,
/// the 16 in-crate tests, and `tests/memory_multiport_arbitration.rs` all check the
/// simulator against a Rust reference; `tests/verilog_fifo_memory_new.rs` anchors to
/// *hand-written* Verilog rather than to transpiled output. So the project's central
/// claim — the same source simulates and synthesises, provably in agreement — simply
/// does not extend to designs using `Memory`. That is worth stating plainly rather
/// than leaving as an absence.
///
/// It also removes the usual way of settling semantics questions here: out-of-range
/// addressing was decided on diagnostic grounds (a deliberate panic naming the port,
/// address and size) precisely because there is no synthesised counterpart to be
/// faithful to. See `copper-core/src/memory.rs::out_of_range`.
#[test]
fn memory_is_not_transpilable() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/memory/dual_port_ram.rs"
    ))
    .expect("read the dual_port_ram example");

    let err = transpile(&src).expect_err(
        "Memory now transpiles! Promote this to a real equivalence test against \
         examples/memory/sv/dual_port_ram.sv, extend P4's from_fn/from_contents \
         preload check through the transpiled path, and revisit the out-of-range \
         decision now that a synthesised counterpart exists to adjudicate against.",
    );
    assert!(
        err.contains("cannot infer bit width"),
        "the Memory transpile gap changed shape; it used to fail bit-width inference. \
         New error: {err}"
    );
}
