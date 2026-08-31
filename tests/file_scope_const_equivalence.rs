//! Equivalence test for a module whose widths and bounds come from **file-scope
//! `const` items** — sim ≡ transpiled SystemVerilog under Verilator.
//!
//! This is the behavioural half of the `localparam` work (the text-level and
//! diagnostic half is `copper-codegen/tests/file_scope_consts.rs`). It matters
//! because a const reaches the emitted module in two structurally different
//! places — a **port width** (`[WIDTH-1:0]`, which is why the declaration has to
//! live in the parameter port list) and a **body expression** (a loop bound and
//! a width cast) — and only Verilating the result proves both are declared
//! before use and mean the same number the simulator used.
//!
//! The design is a one-hot encoder, BaseJump `bsg_encode_one_hot`'s behaviour:
//! `addr_o` = index of the set bit, `v_o` = (input != 0). The reference model
//! below is written independently of the DUT (a plain scan over a `u8`), and the
//! sweep covers every one-hot input, the zero input, and — as the DUT documents
//! "last set bit wins" — a few multi-bit inputs.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::Logic;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

include!("fixtures/file_const_encoder_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/file_const_encoder_dut.rs");

/// Independent model: the index of the highest set bit, and whether any is set.
/// Deliberately not the DUT's formulation — a scan from the top, not a
/// last-writer-wins sweep from the bottom.
fn model(input: u8) -> (usize, bool) {
    for k in (0..8).rev() {
        if input & (1 << k) != 0 {
            return (k, true);
        }
    }
    (0, false)
}

#[test]
fn file_scope_consts_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("const_encoder", DUT_SRC);

    let mut exec = HardwareExecutor::new();
    let (i_drv, i_in) = wire::<Bits<WIDTH>, ()>(Bits::zero());
    let (addr_out, addr_obs) = wire::<Bits<ADDR_W>, ()>(Bits::zero());
    let (v_out, v_obs) = wire::<Logic, ()>(Logic::Zero);
    let dh_a = addr_out.dirty_handle();
    let dh_v = v_out.dirty_handle();
    let reads = vec![i_in.wire_id()];
    exec.spawn_wired(const_encoder(i_in, addr_out, v_out), vec![dh_a, dh_v], reads);

    // Every one-hot input and the zero input, then multi-bit inputs where the
    // highest set bit is the one that survives.
    let mut stimulus: Vec<u8> = vec![0];
    stimulus.extend((0..8).map(|k| 1u8 << k));
    stimulus.extend([0b0000_0011, 0b1000_0001, 0b0101_0100, 0b1111_1111]);

    for input in stimulus {
        i_drv.write(Bits::<WIDTH>::from_u8(input));
        exec.poll_tasks();

        let (addr, valid) = model(input);
        eq.record(
            &[("i", Bits::<WIDTH>::from_u8(input).as_array())],
            &[
                ("addr_o", addr_obs.read().as_array()),
                ("v_o", &[v_obs.read()]),
            ],
            &[
                ("addr_o", Bits::<ADDR_W>::from_usize(addr).as_array()),
                ("v_o", &[if valid { Logic::One } else { Logic::Zero }]),
            ],
        );
    }

    eq.finish();
}
