//! Independent-hardware anchor for the "010" sequence detector (G1).
//!
//! Fills the variable-iteration-loop gap in
//! `design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`: before this, the "010"
//! detector had no independent hardware golden — the only check was two Copper
//! codings (`det_010` vs `det_010_awaits`) against *each other*
//! (`examples/sequential/pattern_detector_2.rs`), which cannot say which
//! cycle-by-cycle behavior is correct.
//!
//! Here the Copper simulator's trace is checked against a hand-written Verilog
//! reference (`examples/sequential/sv/pattern_detector_010.sv`) under Verilator —
//! a different simulator running independently-written hardware. This is the
//! discipline from `CLAUDE.md`: adjudicate "which behavior is correct" against an
//! independent reference, never against the transpiler's own output.
//!
//!   * `det_010` (canonical single-tick Moore coding) — LIVE. Anchors the working
//!     coding to independent hardware.
//!   * `det_010_awaits` (variable-iteration-loop coding) — `#[ignore]`. It
//!     currently diverges from the golden because of the runtime read-timing
//!     heuristic (see the module comment in `pattern_detector_2.rs`). Retiring
//!     that heuristic with CFG-derived static timing (impl-plan item 3) must make
//!     this pass against *this* golden — that is item 3's provable claim.

mod common;

use copper_core::port::{registered_wire, wire, In, Out, RegOut};
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::{make_cycle, HardwareExecutor, HardwareTest, SimulationTrace};

struct MainClk;
impl ClockDomain for MainClk {}

const GOLDEN_SV: &str = "examples/sequential/sv/pattern_detector_010.sv";

// The canonical Moore coding (single source of truth, shared with
// tests/det_010_equivalence.rs).
include!("fixtures/det_010_dut.rs");

// The variable-iteration-loop coding — mirrors `det_010_awaits` in
// examples/sequential/pattern_detector_2.rs. Kept here so the anchor below is
// self-contained; if the example's coding changes, update both.
//
// `out_o` is a `RegOut`, not a plain `Out`: this coding drives its detection
// output *before* a tick (`out_o.write(One); clk.tick().await;`), a write-before-
// tick Moore output — the exact case `RegOut` exists for (CLAUDE.md; verified on
// `sipo_block`). With plain `Out` the next iteration's leading `out_o.write(Zero)`
// clobbers the detection in the same `tick_clock`'s post-edge before it is
// observed. This is the *output*-timing axis; item 3's read-timing classification
// independently makes the machine detect on the correct cycles (5, 9, 13).
#[hardware(sequential)]
async fn det_010_awaits(
    clk: Clock<MainClk>,
    rstn: In<Logic, MainClk>,
    in_i: In<Logic, MainClk>,
    out_o: RegOut<Logic, MainClk>,
) {
    loop {
        out_o.write(Logic::Zero);
        if rstn.read() == Logic::Zero {
            out_o.write(Logic::Zero);
            clk.tick().await;
        } else if in_i.read() == Logic::Zero {
            clk.tick().await;
            while in_i.read() == Logic::Zero {
                clk.tick().await;
            }
            if in_i.read() == Logic::One {
                clk.tick().await;
                if in_i.read() == Logic::Zero {
                    out_o.write(Logic::One);
                    clk.tick().await;
                }
            }
        } else {
            // Idle: not in reset and no leading `0` yet (in_i == 1). Wait one
            // cycle and re-check. Previously this path fell through with *no*
            // tick — a zero-time combinational spin the reachability check now
            // rejects; the missing idle tick is the fix. (Mirrors the coding in
            // examples/sequential/pattern_detector_2.rs — keep both in sync.)
            clk.tick().await;
        }
    }
}

/// Reference model: apply the transition, then `out = (new state == D)`. This is
/// the same 4-state table the golden `.sv` encodes, expressed in Rust so the
/// harness's third leg (sim vs model) is independent of both the sim's execution
/// and the Verilog.
fn next_ref(state: u8, rstn: Logic, input: Logic) -> u8 {
    if rstn == Logic::Zero {
        return 0; // A
    }
    match (state, input) {
        (0, Logic::Zero) => 1, // A,0 -> B
        (1, Logic::One) => 2,  // B,1 -> C
        (1, Logic::Zero) => 1, // B,0 -> B
        (2, Logic::Zero) => 3, // C,0 -> D
        (3, Logic::Zero) => 1, // D,0 -> B
        _ => 0,                // -> A
    }
}

/// Walks every reachable (state, input) transition at least once — including both
/// exits from D and a reset asserted from a non-A state. Same stimulus as the
/// two-codings test in `pattern_detector_2.rs`, so the codings are compared on the
/// identical stream, now against independent hardware rather than each other.
fn coverage_stream() -> Vec<(Logic, Logic)> {
    let z = Logic::Zero;
    let o = Logic::One;
    vec![
        (z, z), // reset          -> A
        (o, o), // A,1 -> A
        (o, z), // A,0 -> B
        (o, z), // B,0 -> B
        (o, o), // B,1 -> C
        (o, z), // C,0 -> D   (out high)
        (o, o), // D,1 -> A
        (o, z), // A,0 -> B
        (o, o), // B,1 -> C
        (o, z), // C,0 -> D   (out high)
        (o, z), // D,0 -> B
        (o, z), // A,0 -> B  (from B after D,0->B this is B,0->B)
        (o, o), // B,1 -> C
        (o, z), // C,0 -> D   (out high)
        (z, o), // reset from D   -> A
        (o, z), // A,0 -> B
        (o, o), // B,1 -> C
        (o, o), // C,1 -> A
    ]
}

/// Run one coding against the independent Verilog golden under Verilator, plus
/// the Rust reference model. `verilog` selects the DUT; both are driven by the
/// same coverage stream.
fn run_against_golden(
    module: &str,
    // The closure spawns the DUT and returns its output observation handle — it
    // owns the output wiring so each coding can pick plain `Out` (combinational,
    // post-tick) or `RegOut` (registered, write-before-tick) as its shape requires.
    spawn: impl FnOnce(
        &mut HardwareExecutor,
        Clock<MainClk>,
        In<Logic, MainClk>,
        In<Logic, MainClk>,
    ) -> In<Logic, MainClk>,
) {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
    let (in_drv, in_in) = wire::<Logic, MainClk>(Logic::Zero);
    let out_obs = spawn(&mut exec, clk.clone(), rstn_in, in_in);

    // The Verilator top module is `det_010` (the golden's module name) for both
    // codings — they are checked against the same reference hardware.
    let _ = module;
    let mut test = HardwareTest::new("det_010").with_verilog(GOLDEN_SV);

    let cases = coverage_stream();
    let mut ref_state = 0u8;
    let mut expected = Vec::new();
    for (i, &(rstn, input)) in cases.iter().enumerate() {
        rstn_drv.write(rstn);
        in_drv.write(input);
        exec.tick_clock(&mut clk);
        ref_state = next_ref(ref_state, rstn, input);
        test.record_cycle(
            i,
            &[("rstn", &[rstn]), ("in", &[input])],
            &[("out", &[out_obs.read()])],
        );
        expected.push(make_cycle(
            i,
            &[("rstn", &[rstn]), ("in", &[input])],
            &[("out", &[Logic::from_bool(ref_state == 3)])],
        ));
    }

    let expected = SimulationTrace::from_cycles(expected);
    test.finish_with_expected(&expected).assert_passed();
}

/// The canonical single-tick Moore coding matches the independent hardware golden
/// cycle-by-cycle under Verilator. This anchors the working coding — the "010"
/// detector now has a real hardware reference.
#[test]
fn det_010_matches_independent_verilog() {
    run_against_golden("det_010", |exec, clk, rstn, in_i| {
        let (out_drv, out_obs) = wire::<Logic, MainClk>(Logic::Zero);
        let dh = out_drv.dirty_handle();
        exec.spawn_wired(det_010(clk, rstn, in_i, out_drv), vec![dh]);
        out_obs
    });
}

/// The variable-iteration-loop coding matches the SAME independent golden
/// cycle-by-cycle under Verilator. This is **impl-plan item 3's provable claim**:
/// replacing the runtime `synced_read` heuristic with CFG-derived static read
/// timing (`copper_analysis::classify_reads`) makes the variable-iteration
/// `while in_i.read() == 0 { tick }` machine sample its inputs on the correct
/// cycles, so it detects the "010" pattern where the independent hardware does.
/// (The output uses `RegOut` — the write-before-tick Moore output axis — see the
/// module note above; that is orthogonal to the read timing this claim is about.)
#[test]
fn det_010_awaits_matches_independent_verilog() {
    run_against_golden("det_010_awaits", |exec, clk, rstn, in_i| {
        let (out_drv, out_obs) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
        let dh = out_drv.dirty_handle();
        exec.spawn_wired(det_010_awaits(clk, rstn, in_i, out_drv), vec![dh]);
        out_obs
    });
}
