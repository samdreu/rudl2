//! Behavioral-equivalence test for a raw `[Logic; N]` array local.
//!
//! This is the **positive** counterpart to what
//! `copper-codegen/tests/transpile_inference_gaps.rs` used to pin as a known
//! gap: `[Logic::Zero; N]` array locals did not transpile ("cannot infer bit
//! width"), so every design written in that idiom — `sipo_block`,
//! `ripple_carry_adder`, `bsg_gray_to_binary`, `bsg_adder_one_hot` — was
//! simulator-only, and the equivalence-tested rewrites used `Bits` indexing to
//! sidestep it.
//!
//! An array local now lowers to the packed vector its length describes (element
//! k at bit k — the same layout `Bits<N>` already has in `copper-core`, where
//! `Bits { bits: [Logic; N] }`), so `Bits::from_slice`/`from_array` move no bits
//! and lower to identity.
//!
//! The golden is independent of both Copper paths: the word is computed here in
//! plain Rust from the four nibbles, so this checks sim ≡ transpiled SV ≡ an
//! outside reference, not just the two Copper artifacts against each other.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::Logic;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

include!("fixtures/logic_array_pack_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/logic_array_pack_dut.rs");

#[test]
fn logic_array_pack_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("logic_array_pack", DUT_SRC);

    let mut exec = HardwareExecutor::new();
    let (n0_drv, n0_in) = wire::<Bits<4>, ()>(Bits::zero());
    let (n1_drv, n1_in) = wire::<Bits<4>, ()>(Bits::zero());
    let (n2_drv, n2_in) = wire::<Bits<4>, ()>(Bits::zero());
    let (n3_drv, n3_in) = wire::<Bits<4>, ()>(Bits::zero());
    let (word_out, word_obs) = wire::<Bits<16>, ()>(Bits::zero());

    let dh = word_out.dirty_handle();
    let reads = vec![
        n0_in.wire_id(),
        n1_in.wire_id(),
        n2_in.wire_id(),
        n3_in.wire_id(),
    ];
    exec.spawn_wired(
        logic_array_pack(n0_in, n1_in, n2_in, n3_in, word_out),
        vec![dh],
        reads,
    );

    // A deterministic sweep over the nibbles: every value of n0 against a
    // rotating set of the others, so each nibble takes every value and lands in
    // its own field (a packing that dropped or misplaced a field would show up
    // as a mismatch against the golden below).
    for i in 0u8..16 {
        let n0 = i;
        let n1 = (i * 3 + 1) & 0xF;
        let n2 = (i * 5 + 2) & 0xF;
        let n3 = (i * 7 + 3) & 0xF;

        n0_drv.write(Bits::<4>::from_usize(n0 as usize));
        n1_drv.write(Bits::<4>::from_usize(n1 as usize));
        n2_drv.write(Bits::<4>::from_usize(n2 as usize));
        n3_drv.write(Bits::<4>::from_usize(n3 as usize));
        exec.poll_tasks();

        // Independent golden: nibble n occupies bits 4n..4n+3.
        let expected = (n0 as u16)
            | ((n1 as u16) << 4)
            | ((n2 as u16) << 8)
            | ((n3 as u16) << 12);

        eq.record(
            &[
                ("n0_i", Bits::<4>::from_usize(n0 as usize).as_array()),
                ("n1_i", Bits::<4>::from_usize(n1 as usize).as_array()),
                ("n2_i", Bits::<4>::from_usize(n2 as usize).as_array()),
                ("n3_i", Bits::<4>::from_usize(n3 as usize).as_array()),
            ],
            &[("word_o", word_obs.read().as_array())],
            &[("word_o", Bits::<16>::from_usize(expected as usize).as_array())],
        );
    }

    eq.finish();
}
