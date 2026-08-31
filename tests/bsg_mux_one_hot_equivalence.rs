#![allow(unused_imports)] // the example's `main` is cfg'd out; its harness imports go with it

//! `bsg_mux_one_hot`: sim ≡ transpiled SystemVerilog, on top of the example's
//! own sim ≡ BaseJump check.
//!
//! The first module with an **array-typed port** to transpile at all. Its
//! `data_i: In<[Bits<WIDTH>; ELS], ()>` emits as a packed 2-D vector,
//! `[ELS-1:0][WIDTH-1:0]` — the same declaration BaseJump's own
//! `bsg_mux_one_hot.sv` uses. See `design_docs/ARRAY_PORT_ABI.md`.
//!
//! This is the **constant-index** half of the array coverage: `d[i]` over a loop
//! variable. `tests/mux_equivalence.rs` covers the dynamic-index half (`d[sel]`),
//! which is the case that actually needs a run-time select.
//!
//! It also exercises an array-typed LOCAL (`let d = data_i.read();`), which needs
//! both packed dimensions on its wire declaration — declared at the element width
//! it would silently truncate, which is how that bug was found.

mod common;

use common::EquivalenceTest;

include!("../examples/basejump/bsg_mux_one_hot.rs");
const SRC: &str = include_str!("../examples/basejump/bsg_mux_one_hot.rs");

#[test]
fn bsg_mux_one_hot_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("bsg_mux_one_hot", SRC);

    let mut exec = HardwareExecutor::new();
    let (data_drv, data_in) = wire::<[Bits<WIDTH>; ELS], ()>([Bits::zero(); ELS]);
    let (sel_drv, sel_in) = wire::<Bits<ELS>, ()>(Bits::zero());
    let (out_o, out_obs) = wire::<Bits<WIDTH>, ()>(Bits::zero());
    let dh = out_o.dirty_handle();
    let reads = vec![data_in.wire_id(), sel_in.wire_id()];
    exec.spawn_wired(bsg_mux_one_hot(data_in, sel_in, out_o), vec![dh], reads);

    // BaseJump's own testbench: data_i[i] = i, sel sweeps 001 -> 010 -> 100.
    let data: [Bits<WIDTH>; ELS] = [
        Bits::from_usize(0),
        Bits::from_usize(1),
        Bits::from_usize(2),
    ];
    // Element 0 at the LSBs — the packed layout Verilator gives the port.
    let data_flat: Vec<Logic> = data.iter().flat_map(|b| b.as_array().iter().copied()).collect();

    for i in 0..ELS {
        let mut sel = Bits::<ELS>::zero();
        sel[i] = Logic::One;
        data_drv.write(data);
        sel_drv.write(sel);
        exec.poll_tasks();

        // Independent model: a one-hot select picks exactly data_i[i].
        eq.record(
            &[("data_i", &data_flat), ("sel_one_hot_i", sel.as_array())],
            &[("data_o", out_obs.read().as_array())],
            &[("data_o", Bits::<WIDTH>::from_usize(i).as_array())],
        );
    }

    // All-zero select: nothing is masked through, so the OR-reduce is zero.
    data_drv.write(data);
    sel_drv.write(Bits::<ELS>::zero());
    exec.poll_tasks();
    eq.record(
        &[("data_i", &data_flat), ("sel_one_hot_i", Bits::<ELS>::zero().as_array())],
        &[("data_o", out_obs.read().as_array())],
        &[("data_o", Bits::<WIDTH>::zero().as_array())],
    );

    eq.finish();
}
