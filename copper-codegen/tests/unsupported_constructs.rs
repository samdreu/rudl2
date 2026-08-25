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

/// A tick inside a nested `loop` — the "wait until ready" idiom — DOES transpile,
/// as a self-looping FSM state (control extraction increment B).
/// `tests/wait_loop_equivalence.rs` at the repo root carries the behavioural
/// checks; this is the control that the shape reaches the flattener at all.
///
/// Historically this is also the shape that used to CRASH the transpiler: the gate
/// and the tick search in `control_extract.rs` disagreed about where a tick can
/// live. `no_transpiler_panics.rs` guards that failure MODE corpus-wide.
#[test]
fn tick_inside_a_nested_loop_is_supported() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, go: In<Logic, MainClk>, o: RegOut<Bits<8>, MainClk>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        o.write(n);
        loop {
            if go.read() == Logic::One { break; }
            clk.tick().await;
        }
        n = n + Bits::from_lit::<1>();
        clk.tick().await;
    }
}
"#;
    let sv = transpile(src).expect("a repeating wait must transpile");
    assert!(
        sv.contains("case (pc)"),
        "a nested wait must flatten to a pc FSM, got:\n{sv}"
    );
}

/// The tick must be the LAST statement of the wait's body. The other ordering is
/// **outside the language by decision** (2026-08-24), not merely unimplemented: it
/// puts the test in the window where an `Immediate` read consumes the value the
/// just-past edge produced while the flip-flop it lowers to samples the value
/// before its own edge. Measured, the transpiled module reacted a full cycle
/// early, and holding the stimulus two cycles did not reconcile them.
///
/// Same disposition as the pre-tick alignment hazard: the divergent program is
/// made unwritable rather than the divergence adjudicated. See
/// `design_docs/SYNCHRONOUS_SEMANTICS.md`.
#[test]
fn ticking_before_the_test_in_a_wait_is_unsupported() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, go: In<Logic, MainClk>, o: RegOut<Bits<8>, MainClk>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        o.write(n);
        loop {
            clk.tick().await;
            if go.read() == Logic::One { break; }
        }
        n = n + Bits::from_lit::<1>();
        clk.tick().await;
    }
}
"#;
    let err = transpile(src).expect_err(
        "This ordering is refused BY DESIGN, not pending support — see \
         SYNCHRONOUS_SEMANTICS.md. If it now transpiles, a language decision was reversed; \
         that needs sign-off and a hardware-anchored check that the module no longer reacts a \
         cycle earlier than the simulator, not just a green test.",
    );
    assert!(
        err.contains("LAST statement"),
        "the diagnostic must state the ordering rule: {err}"
    );
}

/// `continue` is refused: jumping to the loop head mid-cycle needs the head's
/// *lowered* body at a point where it is still being lowered.
#[test]
fn continue_in_a_wait_is_unsupported() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, go: In<Logic, MainClk>, o: RegOut<Bits<8>, MainClk>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        o.write(n);
        loop {
            if go.read() == Logic::Zero { continue; }
            break;
        }
        n = n + Bits::from_lit::<1>();
        clk.tick().await;
    }
}
"#;
    let err = transpile(src).expect_err("`continue` must be refused");
    assert!(
        !err.is_empty() && !err.starts_with("0:0:"),
        "the `continue` diagnostic must carry a span: {err}"
    );
}

/// A nested `loop` with no tick, in a module that does tick elsewhere. Distinct
/// message from the one above because the fix is different — and it has to be its
/// own case: a module with NO tick at all is caught earlier and better by the
/// shared reachability guarantee ("every path through a `#[hardware]` loop must
/// reach `clk.tick().await`"), which is asserted below so the division of labour
/// between the two checks stays visible.
#[test]
fn nested_loop_without_a_tick_is_unsupported() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, o: Out<Bits<8>, MainClk>) {
    loop {
        loop {
            o.write(a.read());
        }
        clk.tick().await;
    }
}
"#;
    let err = transpile(src).expect_err("an unbounded combinational loop must be rejected");
    assert!(
        err.contains("would never terminate"),
        "reproduced a *different* error than the untick'd nested loop: {err}"
    );

    // Same shape, but the module never ticks at all: the reachability guarantee
    // owns this one, and its message is the better of the two.
    let no_tick = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, o: Out<Bits<8>, MainClk>) {
    loop {
        loop {
            o.write(a.read());
        }
    }
}
"#;
    let err = transpile(no_tick).expect_err("a loop with no tick must be rejected");
    assert!(
        err.contains("must reach `clk.tick().await`"),
        "the reachability guarantee should own the no-tick case: {err}"
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
// `examples/memory/sv/dual_port_ram.sv`. Preloaded contents followed on the same
// day (`initial` block; `tests/preloaded_memory_equivalence.rs`), then WriteFirst
// (`tests/write_first_memory_equivalence.rs`), multi-phase access
// (`tests/multiphase_memory_equivalence.rs`) and pipelined latency
// (`tests/pipelined_memory_equivalence.rs`). Every remaining memory test below is
// a RULE — a shape hardware cannot express, or contents the transpiler cannot
// see — rather than an unimplemented construct.
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

/// Read/write latency greater than one cycle DOES transpile: a read port becomes
/// a register chain and a write port's commit comes from its last stage.
/// `tests/pipelined_memory_equivalence.rs` carries the behavioural checks at
/// READ_LAT 2 and 3 and WRITE_LAT 2, including a WriteFirst forward from the
/// committing stage.
#[test]
fn pipelined_memory_latency_is_supported() {
    let src = mem_dut(
        "let mem = Memory::<Bits<16>, 1, 1, MainClk, 2, 1>::new(clk.clone(), 256);",
        MEM_BODY,
    );
    let sv = transpile(&src).expect("a pipelined memory must transpile");
    assert!(
        sv.contains("mem_rd0_q0 <= mem_rd0_data;"),
        "READ_LAT=2 must emit a capture stage, got:\n{sv}"
    );
}

/// Zero latency is not a thing a synchronous port can do, and the simulator's
/// pipelines index `[LAT - 1]`, so it is refused rather than underflowing.
#[test]
fn zero_latency_memory_is_rejected() {
    let src = mem_dut(
        "let mem = Memory::<Bits<16>, 1, 1, MainClk, 0, 1>::new(clk.clone(), 256);",
        MEM_BODY,
    );
    let err = transpile(&src).expect_err("READ_LAT = 0 must be rejected");
    assert!(
        err.contains("must be at least 1"),
        "reproduced a *different* error than the zero-latency rule: {err}"
    );
}

/// WriteFirst read-during-write DOES transpile, as a same-address forwarding mux
/// on the read port's output net. `tests/write_first_memory_equivalence.rs` at the
/// repo root carries the behavioural check, including a differential against
/// ReadFirst on identical stimulus (the two modes agree on every cycle except a
/// same-address read/write, so nothing else can tell them apart).
#[test]
fn write_first_memory_is_supported() {
    let src = mem_dut(
        "let mem = Memory::<Bits<16>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 256).write_first();",
        MEM_BODY,
    );
    let sv = transpile(&src).expect("a WriteFirst memory must transpile");
    assert!(
        sv.contains("mem_wr0_en && (mem_wr0_addr == mem_rd0_addr)"),
        "WriteFirst must forward a same-address write to the read, got:\n{sv}"
    );
}

/// A read-only preloaded memory: the shape the pins below vary one thing from.
fn rom_dut(decl: &str) -> String {
    format!(
        r#"
#[hardware(sequential)]
async fn r(clk: Clock<MainClk>, a: In<Bits<4>, MainClk>, o: Out<Bits<16>, MainClk>) {{
    let rom = {decl};
    let mut q: Bits<16> = Bits::zero();
    loop {{
        rom.read_port::<0>().read(a.read().as_usize());
        clk.tick().await;
        if rom.read_port::<0>().is_ready() {{ q = rom.read_port::<0>().data(); }}
        o.write(q);
    }}
}}
"#
    )
}

/// Preloaded contents DO transpile, as an `initial` block.
/// `tests/preloaded_memory_equivalence.rs` at the repo root carries the
/// behavioural sim ≡ transpiled-SV check for both constructors.
#[test]
fn preloaded_memory_is_supported() {
    let sv = transpile(&rom_dut(
        "Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, |i| Bits::from_usize(i * 3))",
    ))
    .expect("a `from_fn` preload must transpile");
    assert!(
        sv.contains("initial begin") && sv.contains("for (int i = 0; i < 16; i++)"),
        "expected a fill loop in an initial block, got:\n{sv}"
    );
}

/// Contents that only exist at run time. `examples/cpu/rv32i_cpu.rs` is the real
/// instance: `from_contents(clk, flat.clone())`, where `flat` is a program image
/// assembled by the harness. The transpiler does not execute Rust, so there is
/// nothing to emit — and emitting a zero-filled array instead would be a silently
/// wrong design.
#[test]
fn runtime_computed_preload_is_unsupported() {
    let err = transpile(&rom_dut(
        "Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_contents(clk.clone(), flat.clone())",
    ))
    .expect_err(
        "NOW SUPPORTED: a run-time `Vec` preload transpiles — which would mean the transpiler \
         gained a way to evaluate Rust. Check very carefully what it actually emits.",
    );
    assert!(
        err.contains("computed at run time") && err.contains("does not execute Rust"),
        "reproduced a *different* error than the tracked run-time-contents gap: {err}"
    );
}

/// A fill that is not written at the call site — a named function, or a closure
/// bound to a variable. Same root cause as above: there is no body to emit.
#[test]
fn non_inline_fill_function_is_unsupported() {
    let err = transpile(&rom_dut(
        "Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, make_word)",
    ))
    .expect_err("a `from_fn` fill that is not an inline closure must be rejected");
    assert!(
        err.contains("must be a closure written at the call site"),
        "reproduced a *different* error than the inline-fill rule: {err}"
    );
}

/// A preload that reads one of the module's own signals. The simulator would
/// evaluate the captured port ONCE at construction; an `initial` block samples it
/// at time 0. Those are different things that look alike in source, so the shape
/// is rejected rather than emitted.
#[test]
fn preload_reading_a_signal_is_rejected() {
    let err = transpile(&rom_dut(
        "Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, |i| Bits::from_usize(i) + a.read())",
    ))
    .expect_err("a preload that reads a signal must be rejected");
    assert!(
        err.contains("Initial contents must be constant"),
        "reproduced a *different* error than the constant-preload rule: {err}"
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

/// Multi-phase memory DOES transpile: the address buses are phase-gated and the
/// read result is captured into a pipeline register that survives the edge.
/// `tests/multiphase_memory_equivalence.rs` carries the behavioural checks.
#[test]
fn memory_in_a_multi_phase_loop_is_supported() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, o: Out<Bits<16>, MainClk>) {
    let mem = Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::new(clk.clone(), 256);
    let mut q: Bits<16> = Bits::zero();
    loop {
        o.write(q);
        mem.read_port::<0>().read(a.read().as_usize());
        clk.tick().await;
        if mem.read_port::<0>().is_ready() { q = mem.read_port::<0>().data(); }
        clk.tick().await;
    }
}
"#;
    let sv = transpile(src).expect("a two-phase memory read must transpile");
    assert!(
        sv.contains("mem_rd0_q0 <= mem_rd0_data;"),
        "a cross-phase read result must be captured into a pipeline register, got:\n{sv}"
    );
}

/// Combinational statements after the LAST tick of a multi-tick loop. They have
/// no phase to belong to and used to be dropped silently — an output written
/// there simply vanished, leaving an undriven port. Not memory-specific; the
/// memory work is just what surfaced it.
#[test]
fn trailing_combinational_statements_in_a_multi_tick_loop_are_rejected() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, o: Out<Bits<8>, MainClk>,
           t: Out<Bits<8>, MainClk>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = a.read();
        clk.tick().await;
        o.write(r);
        clk.tick().await;
        t.write(r);
    }
}
"#;
    let err = transpile(src).expect_err(
        "NOW SUPPORTED: trailing combinational statements lower. That decides which phase they \
         belong to — a semantics question; make sure it was decided, not defaulted.",
    );
    assert!(
        err.contains("after the last `clk.tick().await`"),
        "reproduced a *different* error than the tracked trailing-segment gap: {err}"
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
        err.contains("is read before the `clk.tick().await` that produces it"),
        "reproduced a *different* error than the read-ordering rule: {err}"
    );
}
