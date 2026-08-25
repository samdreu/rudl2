#![allow(unused_imports)] // the example's `main` is cfg'd out; its harness imports go with it

//! `mux`: sim ≡ transpiled SystemVerilog, on top of the example's own
//! sim ≡ BaseJump check.
//!
//! The **dynamic-index** half of the array-port coverage: `data_i[sel_i]`, a
//! run-time select into an array-typed port. (`bsg_mux_one_hot` covers the
//! constant-index half.) It is also the only module in the corpus whose array
//! port has *symbolic* dimensions in both directions — `WIDTH_P` and `ELS_P` are
//! const generics — which is the case that decided the packed-2-D ABI: each
//! dimension renders as its own parameter reference, so neither needs the width
//! arithmetic `Width` cannot express. See `design_docs/ARRAY_PORT_ABI.md`.
//!
//! The transpiled module is Verilated at the same widths the simulator ran
//! (`with_params`), since it is emitted parametrically.

mod common;

use common::EquivalenceTest;

include!("../examples/combinational/mux.rs");
const SRC: &str = include_str!("../examples/combinational/mux.rs");

#[test]
fn mux_sim_matches_transpiled_verilog() {
    const W: usize = 8;
    const E: usize = 4;
    const LG: usize = 2;

    let mut eq = EquivalenceTest::new("mux", SRC)
        .with_params(&[("WIDTH_P", W as i64), ("ELS_P", E as i64), ("LG_ELS_LP", LG as i64)]);

    let mut exec = HardwareExecutor::new();
    let (data_drv, data_in) = wire::<[Bits<W>; E], ()>([Bits::zero(); E]);
    let (sel_drv, sel_in) = wire::<Bits<LG>, ()>(Bits::zero());
    let (out_o, out_obs) = wire::<Bits<W>, ()>(Bits::zero());
    let dh = out_o.dirty_handle();
    let reads = vec![data_in.wire_id(), sel_in.wire_id()];
    exec.spawn_wired(mux::<W, E, LG>(data_in, sel_in, out_o), vec![dh], reads);

    // Distinct values so any selection error is visible rather than cancelling.
    let data: [Bits<W>; E] = [
        Bits::from_u8(0xAA),
        Bits::from_u8(0xBB),
        Bits::from_u8(0xCC),
        Bits::from_u8(0xDD),
    ];
    let expect: [u8; E] = [0xAA, 0xBB, 0xCC, 0xDD];
    // Element 0 at the LSBs — the packed layout Verilator gives the port.
    let data_flat: Vec<Logic> = data.iter().flat_map(|b| b.as_array().iter().copied()).collect();

    for sel in 0..E {
        let sel_bits = Bits::<LG>::from_usize(sel);
        data_drv.write(data);
        sel_drv.write(sel_bits);
        exec.poll_tasks();

        // Independent model: selecting `sel` yields the `sel`th input, read off
        // the stimulus table rather than from the DUT.
        eq.record(
            &[("data_i", &data_flat), ("sel_i", sel_bits.as_array())],
            &[("data_o", out_obs.read().as_array())],
            &[("data_o", Bits::<W>::from_u8(expect[sel]).as_array())],
        );
    }

    eq.finish();
}
