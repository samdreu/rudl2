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

// ── Memory: what lowers, and where the boundary is (P4) ──────────────────────
//
// `Memory` used to be listed here as a whole feature with NO transpiled path —
// an entire first-class construct that existed only in the simulator, so every
// memory guarantee was sim-only and the project's central claim (one source both
// simulates and synthesises, provably in agreement) did not extend to designs
// using it.
//
// As of 2026-08-24 the single-cycle ReadFirst form lowers, and
// `tests/dual_port_ram_equivalence.rs` carries sim ≡ transpiled SV for the
// shipped example — which in turn is checked against an independent hand-written
// `examples/memory/sv/dual_port_ram.sv`. What remains is pinned below, one test
// per construct, each measured against the baseline that does transpile.
//
// One decision still rests on the absence of a synthesised counterpart:
// out-of-range addressing is a deliberate panic naming the port, address and size
// (`copper-core/src/memory.rs::out_of_range`). That was decided on diagnostic
// grounds because nothing existed to adjudicate against. An emitted array now
// exists, so it is worth revisiting — SystemVerilog reads X out of range.

/// A `Memory<..>` DUT that differs from the transpilable baseline in exactly one
/// way. Every pin below shares this shape so the *only* reason each fails is the
/// construct it names.
fn mem_dut(decl: &str, body: &str) -> String {
    format!(
        r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, d: In<Bits<16>, MainClk>,
           we: In<Logic, MainClk>, o: Out<Bits<16>, MainClk>) {{
    {decl}
    let mut q: Bits<16> = Bits::zero();
    loop {{
        {body}
        clk.tick().await;
        if mem.read_port::<0>().is_ready() {{ q = mem.read_port::<0>().data(); }}
        o.write(q);
    }}
}}
"#
    )
}

const MEM_DECL: &str = "let mem = Memory::<Bits<16>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 256);";
const MEM_BODY: &str = r#"
        if we.read() == Logic::One { mem.write_port::<0>().write(a.read().as_usize(), d.read()); }
        mem.read_port::<0>().read(a.read().as_usize());
"#;

/// The control for every memory pin below: this exact shape DOES transpile, so a
/// failure in one of them is about the construct it changes and nothing else.
/// `tests/dual_port_ram_equivalence.rs` at the repo root carries the behavioural
/// sim ≡ transpiled-SV check for the real example.
#[test]
fn single_cycle_readfirst_memory_is_supported() {
    let sv = transpile(&mem_dut(MEM_DECL, MEM_BODY))
        .expect("the baseline memory shape must transpile — every pin below assumes it");
    assert!(
        sv.contains("logic [15:0] mem [0:255];"),
        "expected a packed memory array, got:\n{sv}"
    );
}

/// Read/write latency greater than one cycle. The simulator's pipelines are real
/// behaviour (`copper-core/src/memory.rs` has the LAT=2 tests); the emitted array
/// has no stage registers, so a deeper pipeline is refused rather than flattened.
#[test]
fn memory_latency_above_one_is_unsupported() {
    let src = mem_dut(
        "let mem = Memory::<Bits<16>, 1, 1, MainClk, 2, 1>::new(clk.clone(), 256);",
        MEM_BODY,
    );
    let err = transpile(&src).expect_err(
        "NOW SUPPORTED: pipelined memory latency transpiles. Give it an equivalence test that \
         drives a read every cycle and checks the result appears READ_LAT edges later.",
    );
    assert!(
        err.contains("READ_LAT = 2") && err.contains("single-cycle"),
        "reproduced a *different* error than the tracked latency gap: {err}"
    );
}

/// WriteFirst read-during-write. ReadFirst falls out of non-blocking assignment
/// for free; WriteFirst needs explicit same-address bypass logic.
#[test]
fn write_first_memory_is_unsupported() {
    let src = mem_dut(
        "let mem = Memory::<Bits<16>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 256).write_first();",
        MEM_BODY,
    );
    let err = transpile(&src).expect_err(
        "NOW SUPPORTED: WriteFirst transpiles. Add an equivalence test whose stimulus reads and \
         writes the SAME address on one edge — the only cycle where the two modes differ.",
    );
    assert!(
        err.contains("WriteFirst") && err.contains("ReadFirst"),
        "reproduced a *different* error than the tracked WriteFirst gap: {err}"
    );
}

/// Preloaded contents (`from_fn` / `from_contents`). Named in the `TODO` P4 item:
/// preload equivalence through the transpiled path is still open, now for want of
/// an emitted form rather than for want of memory support at all.
#[test]
fn preloaded_memory_is_unsupported() {
    let src = mem_dut(
        "let mem = Memory::<Bits<16>, 1, 1, MainClk, 1, 1>::from_fn(clk.clone(), 256, |i| Bits::from_usize(i));",
        MEM_BODY,
    );
    let err = transpile(&src).expect_err(
        "NOW SUPPORTED: preloaded memory transpiles. Close P4's from_fn/from_contents preload \
         check through the transpiled path.",
    );
    assert!(
        err.contains("from_fn") && err.contains("not supported"),
        "reproduced a *different* error than the tracked preload gap: {err}"
    );
}

/// Two accesses to one port in a single cycle. The simulator silently keeps the
/// last one (it overwrites pipeline stage 0); one physical address bus cannot.
#[test]
fn two_accesses_to_one_memory_port_are_rejected() {
    let src = mem_dut(
        MEM_DECL,
        &format!("{MEM_BODY}\n        mem.write_port::<0>().write(0, d.read());"),
    );
    let err = transpile(&src).expect_err("a port driven twice in one cycle must be rejected");
    assert!(
        err.contains("accessed 2 times in one cycle"),
        "reproduced a *different* error than the double-drive rule: {err}"
    );
}

/// Multi-phase memory access. A second tick would need phase-guarded address
/// buses and a read result that survives into a later phase; neither exists.
#[test]
fn memory_in_a_multi_phase_loop_is_unsupported() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, o: Out<Bits<16>, MainClk>) {
    let mem = Memory::<Bits<16>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 256);
    let mut q: Bits<16> = Bits::zero();
    loop {
        mem.read_port::<0>().read(a.read().as_usize());
        clk.tick().await;
        if mem.read_port::<0>().is_ready() { q = mem.read_port::<0>().data(); }
        o.write(q);
        clk.tick().await;
    }
}
"#;
    let err = transpile(src).expect_err(
        "NOW SUPPORTED: multi-phase memory transpiles. The phase guard on the address buses and \
         the cross-phase read result both need equivalence coverage.",
    );
    assert!(
        err.contains("exactly one `clk.tick().await`"),
        "reproduced a *different* error than the multi-phase gap: {err}"
    );
}

/// The read result observed on the wrong side of the tick. `data()` is what the
/// clock edge produced, so reading it before the edge would shift the design by a
/// cycle — silently, since the value is well-typed either way.
#[test]
fn memory_read_result_before_the_tick_is_rejected() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, o: Out<Bits<16>, MainClk>) {
    let mem = Memory::<Bits<16>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 256);
    let mut q: Bits<16> = Bits::zero();
    loop {
        mem.read_port::<0>().read(a.read().as_usize());
        q = mem.read_port::<0>().data();
        o.write(q);
        clk.tick().await;
    }
}
"#;
    let err = transpile(src).expect_err("observing a read result before the edge must be rejected");
    assert!(
        err.contains("is read before `clk.tick().await`"),
        "reproduced a *different* error than the read-ordering rule: {err}"
    );
}
