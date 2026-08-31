//! REGRESSION: a plain `Out` written in only ONE phase of a multi-phase loop
//! must HOLD, not drop to 0.
//!
//! ## The bug this pins (found 2026-08-24, fixed the same day)
//!
//! `vlir_lower::lower_flat_stmts` turned a top-level `SHIRStmt::PortDrive` into a
//! module-level `VLIRContinuousAssign`, which carries NO phase guard. The value
//! it assigned was a phase-gated combinational wire defaulting to `'0`, so the
//! port read 0 on every cycle outside its phase:
//!
//! ```text
//! assign w0_dbg = w0;          // module level, unguarded
//! always_comb begin
//!     w0 = '0;                 // ...and w0 is only set in phase 0
//!     if (phase_r == 2'd0) w0 = data_i;
//! ```
//!
//! Sim gave `1,1,3,3` (holding); the transpiled SV gave `0,2,0,4`.
//!
//! ## Why the simulator is the reference here
//!
//! A sequential plain `Out` HOLDS when unwritten — the enabled-register idiom,
//! verified against BaseJump's `bsg_dff_en`. So a write in one phase of a
//! multi-phase loop is *effectively* conditional and must become an implicit-hold
//! register, exactly like a write on some-but-not-all *paths*.
//!
//! ## The fix, and why it needed more than the `RegOut` patch
//!
//! The `RegOut` half of this (`tests/regout_multiphase_equivalence.rs`) was fixed
//! by routing registered outputs to a `PortAssign` so `split_output_regs` moves
//! them into `always_ff`. That alone was NOT enough here: `conditional_output_ports`
//! decides conditionality by finding writes nested in a conditional, and a
//! phase-top-level write is unconditional *within its phase*, so a plain `Out`
//! would still not be recognised.
//!
//! So `lower_seq` also computes `phase_scoped_output_ports` from the SHIR, before
//! the split. It must be done at SHIR level: the VLIR-level
//! `ports_driven_any_path` cannot see an unconditional drive, because by then it
//! has already been moved out of the statement list.
//!
//! ## The rule keys on the VALUE, not on phase coverage — `mac_pipeline` is why
//!
//! The first attempt used "written in some phases but not all". That is WRONG and
//! `tests/mac_pipeline_equivalence.rs` is the measured witness: it lagged that
//! module's output by a cycle (`expected 10 got 0`, `expected 37 got 10`).
//!
//! A top-level drive lowers to a module-level continuous `assign` with no phase
//! guard, so it reads its right-hand side on every cycle. Whether that is correct
//! depends on what the right-hand side is OUTSIDE the writing phase:
//!
//! * `mac_pipeline`: `out.write(sum)` reads the inferred REGISTER `sum_r`, which
//!   retains its value — `assign out = sum_r` tracks it correctly, and registering
//!   it again just adds a cycle.
//! * here / `sipo_block`: `seen_o.write(v)` reads a PHASE-LOCAL COMBINATIONAL WIRE
//!   that `always_comb` defaults to `'0` — so the continuous assign propagates
//!   zeros.
//!
//! Only the second needs the hold. **Deliberately narrow on two counts:** a port
//! written in EVERY phase is excluded (it is driven every cycle), and so is a port
//! whose value is register-backed. Widening either one lags an output by a cycle.
//!
//! ## Blast radius
//!
//! Found via `examples/basejump/sipo_block.rs`, whose four `wN_dbg` ports have
//! this shape. `tests/sipo_block_equivalence.rs` now compares all five outputs.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/plain_out_multiphase_dut.rs");
const SRC: &str = include_str!("fixtures/plain_out_multiphase_dut.rs");

#[test]
fn plain_out_multiphase_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("plain_out_multiphase", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (d_drv, d_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (s_out, s_obs) = wire::<Bits<4>, MainClk>(Bits::zero());
    let dh = s_out.dirty_handle();
    exec.spawn_wired(
        plain_out_multiphase(clk.clone(), d_in.clone(), s_out),
        vec![dh],
        vec![d_in.wire_id()],
    );

    for cycle in 0u8..8 {
        let v = (cycle + 1) & 0xF;
        d_drv.write(Bits::<4>::from_usize(v as usize));
        exec.tick_clock(&mut clk);
        eq.record(
            &[("data_i", Bits::<4>::from_usize(v as usize).as_array())],
            &[("seen_o", s_obs.read().as_array())],
            &[],
        );
    }

    eq.finish();
}

/// Pin the SHAPE as well as the behaviour: a held output must be a non-blocking
/// assign in `always_ff`, never an unguarded module-level continuous assign.
#[test]
fn phase_scoped_out_is_a_hold_register_not_a_continuous_assign() {
    let sv = copper_codegen::transpile_source(
        SRC,
        Some("plain_out_multiphase"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("fixture should transpile");

    assert!(
        sv.contains("seen_o <="),
        "a phase-scoped Out must be driven from always_ff, got:\n{sv}"
    );
    assert!(
        !sv.contains("assign seen_o"),
        "a phase-scoped Out must NOT be an unguarded continuous assign — this is \
         the exact regression this file exists to catch, got:\n{sv}"
    );
}
