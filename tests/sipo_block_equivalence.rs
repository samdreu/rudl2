//! `sipo_block`: sim ≡ transpiled SystemVerilog, on top of the example's own
//! sim ≡ BaseJump check.
//!
//! This module is the one place three separate fixes meet, and it could not
//! transpile at all until all three landed (2026-08-24):
//!   1. it declares `clk: copper_core::Clock<MainClk>` — the only example using a
//!      fully-qualified type path, which every textual port test rejected;
//!   2. it assembles its output through a `[Logic::Zero; 16]` array local, which
//!      failed bit-width inference; and
//!   3. its `data_o` is an unconditionally-written `RegOut`, which lowered to a
//!      continuous `assign` instead of a flip-flop (see
//!      `tests/regout_multiphase_equivalence.rs` for that one in isolation).
//!
//! Why it is worth a test of its own: `examples/basejump/sipo_block.rs` already
//! checks the simulator against BaseJump's independent `sipo_block.sv`. Adding
//! sim ≡ transpiled-SV here chains the two, so the generated SystemVerilog is
//! anchored — transitively — to hardware neither Copper nor its transpiler wrote.
//!
//! The example file itself is `include!`d rather than copied into a fixture, so
//! there is exactly one source of truth for the module and the two checks cannot
//! drift apart.
//!
//! ## Scope: all five outputs
//!
//! Both the `RegOut` (`data_o`) and the four `wN_dbg` plain `Out` debug ports are
//! compared. The `wN_dbg` ports needed a third fix — a plain `Out` written in
//! only some phases of a multi-phase loop must hold, not drop to 0 (see
//! `tests/plain_out_multiphase_hold.rs`). Comparing them matters: they expose the
//! module's internal per-cycle capture, which is exactly what the mid-phase-read
//! question this example was built to answer turns on.

mod common;

use common::EquivalenceTest;

include!("../examples/basejump/sipo_block.rs");
const SRC: &str = include_str!("../examples/basejump/sipo_block.rs");

#[test]
fn sipo_block_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("sipo_block", SRC, Some("sipo_block"));

    let mut clk = copper_core::Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (d_drv, d_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (o_drv, o_obs) = registered_wire::<Bits<16>, MainClk>(&clk, Bits::zero());
    let (w0_o, w0_obs) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (w1_o, w1_obs) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (w2_o, w2_obs) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (w3_o, w3_obs) = wire::<Bits<4>, MainClk>(Bits::zero());

    let dirties = vec![
        o_drv.dirty_handle(),
        w0_o.dirty_handle(),
        w1_o.dirty_handle(),
        w2_o.dirty_handle(),
        w3_o.dirty_handle(),
    ];
    exec.spawn_wired(
        sipo_block(clk.clone(), d_in.clone(), o_drv, w0_o, w1_o, w2_o, w3_o),
        dirties,
        vec![d_in.wire_id()],
    );

    // Three full blocks plus a partial one, with a distinct nibble every cycle so
    // a dropped, duplicated or misplaced word cannot cancel out.
    for cycle in 0u8..14 {
        let v = (cycle * 5 + 1) & 0xF;
        d_drv.write(Bits::<4>::from_usize(v as usize));
        exec.tick_clock(&mut clk);
        eq.record(
            &[("data_i", Bits::<4>::from_usize(v as usize).as_array())],
            &[
                ("data_o", o_obs.read().as_array()),
                ("w0_dbg", w0_obs.read().as_array()),
                ("w1_dbg", w1_obs.read().as_array()),
                ("w2_dbg", w2_obs.read().as_array()),
                ("w3_dbg", w3_obs.read().as_array()),
            ],
            &[],
        );
    }

    eq.finish();
}
