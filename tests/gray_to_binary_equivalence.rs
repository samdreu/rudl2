//! Behavioral-equivalence test for an 8-bit Gray→binary decoder.
//!
//! Promotes the `examples/basejump/bsg_gray_to_binary` self-check into the
//! `cargo test` suite. Concrete width (no params), swept over the whole 256-value
//! input space as `gray = count ^ (count >> 1)`, with the independent golden
//! `binary_o == count`. The generated SystemVerilog is Verilated against the
//! simulator's trace across every input.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::Logic;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

include!("fixtures/gray_to_binary_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/gray_to_binary_dut.rs");

#[test]
fn gray_to_binary_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("gray_to_binary", DUT_SRC);

    let mut exec = HardwareExecutor::new();
    let (g_drv, g_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (b_out, b_obs) = wire::<Bits<8>, ()>(Bits::zero());
    let dh = b_out.dirty_handle();
    let reads = vec![g_in.wire_id()];
    exec.spawn_wired(gray_to_binary(g_in, b_out), vec![dh], reads);

    for count in 0u16..256 {
        let gray = (count ^ (count >> 1)) as u8;
        g_drv.write(Bits::<8>::from_u8(gray));
        exec.poll_tasks();

        let expected = Bits::<8>::from_u8(count as u8); // decode(gray) == count
        eq.record(
            &[("gray_i", Bits::<8>::from_u8(gray).as_array())],
            &[("binary_o", b_obs.read().as_array())],
            &[("binary_o", expected.as_array())],
        );
    }

    eq.finish();
}
