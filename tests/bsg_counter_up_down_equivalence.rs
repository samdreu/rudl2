#![allow(unused_imports)] // the example's `main` is cfg'd out; its harness imports go with it

//! `bsg_counter_up_down`: sim ≡ transpiled SystemVerilog, on top of the
//! example's own sim ≡ BaseJump check.
//!
//! This module needed three separate fixes before it could transpile, all
//! 2026-08-24, each one masking the next:
//!   1. `MOD` and `PTR_W` are file-scope `const` items, which had no lowering
//!      (`undefined variable 'MOD'`) until they became `localparam`s;
//!   2. its `up`/`down` temporaries are branch-local `let`s with computed
//!      initializers, which the latch check rejected — Rust scopes them to the
//!      branch, so they cannot latch, but SystemVerilog needs a default;
//!   3. the counter itself was a bare `usize`, i.e. a 32-bit signal driving a
//!      3-bit port — a width truncation Verilator rejects under `-Wall`. Typing
//!      it `Bits<PTR_W>` also removed the `+ MOD … % MOD` dance, which existed
//!      only to keep a `usize` from underflowing, leaving BaseJump's own
//!      recurrence: `count - down + up`, wrapping at 3 bits.
//!
//! Why it earns a test of its own: the example already checks the simulator
//! against BaseJump's independent Verilog. Adding sim ≡ transpiled-SV chains the
//! two, so the generated SystemVerilog is anchored — transitively — to hardware
//! neither Copper nor its transpiler wrote. The example file is `include!`d
//! rather than copied into a fixture, so the two checks cannot drift apart.
//!
//! The stimulus below covers all four (up, down) combinations, including the
//! both-set case (a net hold, where the two temporaries are live at once) and a
//! wrap in each direction past the 3-bit boundary.

mod common;

use common::EquivalenceTest;

include!("../examples/basejump/bsg_counter_up_down.rs");
const SRC: &str = include_str!("../examples/basejump/bsg_counter_up_down.rs");

#[test]
fn bsg_counter_up_down_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("bsg_counter_up_down", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (rst_drv, rst_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (up_drv, up_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (dn_drv, dn_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_o, out_obs) = wire::<Bits<PTR_W>, MainClk>(Bits::zero());
    let dh = out_o.dirty_handle();
    let reads = vec![rst_in.wire_id(), up_in.wire_id(), dn_in.wire_id()];
    exec.spawn_wired(
        bsg_counter_up_down(clk.clone(), rst_in, up_in, dn_in, out_o),
        vec![dh],
        reads,
    );

    // (reset, up, down). Climb past the 3-bit wrap, hold, descend past zero, and
    // exercise up&down together (net hold) and reset overriding both.
    let cases: &[(bool, bool, bool)] = &[
        (true, false, false),
        (false, true, false), (false, true, false), (false, true, false),
        (false, true, false), (false, true, false), (false, true, false),
        (false, true, false), (false, true, false), // wraps 7 -> 0
        (false, false, false),                      // hold
        (false, true, true),                        // both: net hold
        (false, false, true), (false, false, true), // descends, wraps below 0
        (true, true, true),                         // reset wins
        (false, false, true),
    ];

    // Independent model: BaseJump's recurrence in plain `usize`, wrapped by hand.
    let mut model: usize = 0;

    for &(rst, up, dn) in cases {
        rst_drv.write(Logic::from_bool(rst));
        up_drv.write(Logic::from_bool(up));
        dn_drv.write(Logic::from_bool(dn));
        exec.tick_clock(&mut clk);

        model = if rst { 0 } else { (model + MOD - dn as usize + up as usize) % MOD };

        let (r, u, d) = (Logic::from_bool(rst), Logic::from_bool(up), Logic::from_bool(dn));
        eq.record(
            &[
                ("reset_i", std::slice::from_ref(&r)),
                ("up_i", std::slice::from_ref(&u)),
                ("down_i", std::slice::from_ref(&d)),
            ],
            &[("count_o", out_obs.read().as_array())],
            &[("count_o", Bits::<PTR_W>::from_usize(model).as_array())],
        );
    }

    eq.finish();
}
