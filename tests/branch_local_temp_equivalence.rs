//! Equivalence test for **branch-local temporaries with computed initializers** —
//! sim ≡ transpiled SystemVerilog under Verilator.
//!
//! A `let` inside a branch is scoped to that branch in Rust, so nothing outside
//! can observe it and it cannot latch. The transpiler nonetheless rejected the
//! shape — "would infer a latch: dn, up assigned on some control paths but not
//! all" — because the branch-local default hoist only handled *literal*
//! initializers, moving the whole assignment to the top. A computed initializer
//! cannot move (it would be evaluated on paths the source never runs it on), so
//! it now leaves a zero default behind and stays where it was written.
//! `examples/basejump/bsg_counter_up_down.rs` is the real instance.
//!
//! The DUT deliberately pairs the two shapes that must be treated *differently*:
//!
//! * `up` / `dn` — branch-local temporaries, which get an unconditional default;
//! * `count` — a register driven conditionally, which is the implicit-HOLD idiom
//!   (verified elsewhere against BaseJump's `bsg_dff_en`) and must NOT be
//!   defaulted. Giving it one would clear it on every untaken path, which the
//!   hold cycles in the stimulus below would catch immediately.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/branch_local_temp_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/branch_local_temp_dut.rs");

#[test]
fn branch_local_temporaries_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("branch_local_counter", DUT_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (rst_drv, rst_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (up_drv, up_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (dn_drv, dn_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_out, out_obs) = wire::<Bits<4>, MainClk>(Bits::zero());
    let dh = out_out.dirty_handle();
    let reads = vec![rst_in.wire_id(), up_in.wire_id(), dn_in.wire_id()];
    exec.spawn_wired(
        branch_local_counter(clk.clone(), rst_in, up_in, dn_in, out_out),
        vec![dh],
        reads,
    );

    // (rst, up, dn). The `(0,0,0)` cycles are the ones that would fail if the
    // register were given an unconditional default instead of holding, and the
    // `(0,1,1)` cycles exercise both temporaries being read in one pass.
    let cases: &[(bool, bool, bool)] = &[
        (true, false, false),  // reset
        (false, true, false),  // +1
        (false, true, false),  // +1
        (false, false, false), // hold
        (false, false, true),  // -1
        (false, true, true),   // +1 then -1 — net hold, both temporaries live
        (false, false, false), // hold
        (false, true, false),  // +1
        (true, true, true),    // reset wins over both
        (false, false, true),  // -1, wraps below zero
    ];

    // Independent model: a wrapping 4-bit counter, written from the DUT's
    // documented behaviour rather than traced from its output.
    let mut model: u8 = 0;

    for &(rst, up, dn) in cases {
        let bit = |b: bool| if b { Logic::One } else { Logic::Zero };
        rst_drv.write(bit(rst));
        up_drv.write(bit(up));
        dn_drv.write(bit(dn));
        exec.tick_clock(&mut clk);

        model = if rst {
            0
        } else {
            let mut m = model;
            if up {
                m = m.wrapping_add(1);
            }
            if dn {
                m = m.wrapping_sub(1);
            }
            m & 0xF
        };

        eq.record(
            &[("rst_i", &[bit(rst)]), ("up_i", &[bit(up)]), ("dn_i", &[bit(dn)])],
            &[("count_o", out_obs.read().as_array())],
            &[("count_o", Bits::<4>::from_usize(model as usize).as_array())],
        );
    }

    eq.finish();
}
