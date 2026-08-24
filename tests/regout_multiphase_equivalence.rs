//! REGRESSION: a `RegOut` written UNCONDITIONALLY in a multi-phase loop must be
//! a flip-flop, not a continuous `assign`.
//!
//! ## The bug this pins (found 2026-08-24, fixed the same day)
//!
//! `vlir_lower::lower_flat_stmts` turned every top-level `SHIRStmt::PortDrive`
//! into a module-level `VLIRContinuousAssign`. Those assigns are collected
//! separately from the phase's statements and never passed through
//! `split_output_regs`, so the port's `registered` flag was silently ignored for
//! *unconditional* writes — and in a multi-phase module the phase guard was lost
//! with it. The output then followed a phase-gated combinational value and
//! collapsed to 0 on the cycles where that phase was inactive:
//!
//! ```text
//! FAIL: Cycle 0 word_o expected 0  got 17
//! FAIL: Cycle 1 word_o expected 33 got 0
//! ```
//!
//! A `RegOut` written *inside a conditional* was unaffected — it lowered to a
//! `PortAssign` in the phase statements, which `split_output_regs` does see.
//! That is why `mac_fsm` was always correct and this shape was not.
//!
//! The fix routes a `PortDrive` targeting a registered output to a `PortAssign`
//! instead of a continuous assign, so the existing split moves it into
//! `always_ff` under the phase guard.
//!
//! ## Why the simulator is the reference here
//!
//! The sim holds the registered value between writes — the semantics
//! `examples/basejump/sipo_block` checks against BaseJump's own `sipo_block.sv`,
//! which registers `data_o` in an `always_ff`. The simulator and the third-party
//! hardware agreed; the transpiler was the odd one out.
//!
//! ## Scope (measured, not argued)
//!
//! - NOT about `[Logic; N]` array locals: the array-local form failed on the same
//!   cycles with the same values. This fixture uses a plain `Bits` local so the
//!   two concerns stay pinned apart.
//! - Was UNREACHABLE before the `[Logic; N]` bit-width-inference gap closed —
//!   `sipo_block` failed inference before it ever got this far, which is why a
//!   whole-corpus-green regression never caught it.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/regout_multiphase_dut.rs");
const SRC: &str = include_str!("fixtures/regout_multiphase_dut.rs");

#[test]
fn regout_multiphase_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("regout_multiphase", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (d_drv, d_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (w_out, w_obs) = registered_wire(&clk, Bits::<8>::zero());
    let dh = w_out.dirty_handle();
    let reads = vec![d_in.wire_id()];
    exec.spawn_wired(regout_multiphase(clk.clone(), d_in, w_out), vec![dh], reads);

    for cycle in 0u8..8 {
        let v = (cycle + 1) & 0xF;
        d_drv.write(Bits::<4>::from_usize(v as usize));
        exec.tick_clock(&mut clk);
        eq.record(
            &[("data_i", Bits::<4>::from_usize(v as usize).as_array())],
            &[("word_o", w_obs.read().as_array())],
            &[],
        );
    }

    eq.finish();
}

/// The behavioural check above would also pass if the output happened to agree by
/// luck on the sampled cycles. Pin the SHAPE too: a registered output must be a
/// non-blocking assign inside `always_ff`, never a module-level continuous assign.
#[test]
fn regout_is_a_flipflop_not_a_continuous_assign() {
    let sv = copper_codegen::transpile_source(
        SRC,
        Some("regout_multiphase"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("fixture should transpile");

    assert!(
        sv.contains("word_o <="),
        "a RegOut must be driven from always_ff, got:\n{sv}"
    );
    assert!(
        !sv.contains("assign word_o"),
        "a RegOut must NOT be a continuous assign — this is the exact regression \
         this file exists to catch, got:\n{sv}"
    );
}
