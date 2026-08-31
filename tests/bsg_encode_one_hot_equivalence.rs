//! `bsg_encode_one_hot`: sim ≡ transpiled SystemVerilog, on top of the example's
//! own sim ≡ BaseJump check.
//!
//! Until 2026-08-24 this module could not transpile at all: its width and its
//! loop bound come from file-scope `const` items, which had no lowering
//! (`undefined variable 'WIDTH'`). With those emitted as `localparam`s it
//! transpiles, and narrowing the accumulator from a bare `usize` to
//! `Bits<ADDR_W>` made the result lint-clean — a `usize` local is a 32-bit
//! signal, so driving the 3-bit `addr_o` from it was a width truncation
//! Verilator rejects under `-Wall`.
//!
//! Why it earns a test of its own: `examples/basejump/bsg_encode_one_hot.rs`
//! already checks the simulator against BaseJump's independent Verilog. Adding
//! sim ≡ transpiled-SV chains the two, so the generated SystemVerilog is
//! anchored — transitively — to hardware neither Copper nor its transpiler
//! wrote. The example file is `include!`d rather than copied into a fixture, so
//! the two checks cannot drift apart.

#![allow(unused_imports)] // the example's `main` is cfg'd out; its harness imports go with it

mod common;

use common::EquivalenceTest;

include!("../examples/basejump/bsg_encode_one_hot.rs");
const SRC: &str = include_str!("../examples/basejump/bsg_encode_one_hot.rs");

#[test]
fn bsg_encode_one_hot_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("bsg_encode_one_hot", SRC);

    let mut exec = HardwareExecutor::new();
    let (i_drv, i_in) = wire::<Bits<WIDTH>, ()>(Bits::zero());
    let (addr_o, addr_obs) = wire::<Bits<ADDR_W>, ()>(Bits::zero());
    let (v_o, v_obs) = wire::<Logic, ()>(Logic::Zero);
    let dirties = vec![addr_o.dirty_handle(), v_o.dirty_handle()];
    exec.spawn_wired(bsg_encode_one_hot(i_in.clone(), addr_o, v_o), dirties, vec![i_in.wire_id()]);

    // Every one-hot input and the zero case, following BaseJump's own testbench
    // (a single 1 shifted from bit 0 upward, plus all-zero).
    let mut stimulus: Vec<u8> = vec![0];
    stimulus.extend((0..WIDTH).map(|k| 1u8 << k));

    for input in stimulus {
        i_drv.write(Bits::<WIDTH>::from_u8(input));
        exec.poll_tasks();

        // Independent model: index of the highest set bit, and whether any is set.
        let (addr, valid) = (0..8)
            .rev()
            .find(|k| input & (1 << k) != 0)
            .map_or((0usize, false), |k| (k, true));

        eq.record(
            &[("i", Bits::<WIDTH>::from_u8(input).as_array())],
            &[("addr_o", addr_obs.read().as_array()), ("v_o", &[v_obs.read()])],
            &[
                ("addr_o", Bits::<ADDR_W>::from_usize(addr).as_array()),
                ("v_o", &[if valid { Logic::One } else { Logic::Zero }]),
            ],
        );
    }

    eq.finish();
}
