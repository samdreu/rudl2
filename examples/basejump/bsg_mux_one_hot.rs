// Copper re-implementation of BaseJump STL `bsg_mux_one_hot`, checked for
// equivalence against the original BaseJump Verilog under Verilator
// (examples/basejump/sv/bsg_mux_one_hot.sv). A one-hot select masks each input and
// the results are OR-reduced, so a valid one-hot `sel` picks exactly one input.
//
// Parameters and stimulus are ported from BaseJump's own testbench
// testing/bsg_misc/bsg_mux_one_hot/test_bsg.sv: width_p=4, els_p=3, data_i[i]=i,
// and sel_one_hot_i sweeps 001 -> 010 -> 100 (one bit set, shifting left). Its
// golden check is `data_o == index(sel)`, which is what we assert here.

use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Logic};
use copper_macros::hardware;
use copper_sim::{make_cycle, HardwareExecutor, HardwareTest, SimulationTrace};

const WIDTH: usize = 4;
const ELS: usize = 3;

// data_o = OR over i of (data_i[i] masked by sel_one_hot_i[i]).
#[hardware(combinational)]
fn bsg_mux_one_hot(
    data_i: In<[Bits<WIDTH>; ELS], ()>,
    sel_one_hot_i: In<Bits<ELS>, ()>,
    data_o: Out<Bits<WIDTH>, ()>,
) {
    let d = data_i.read();
    let sel = sel_one_hot_i.read();
    let mut acc: usize = 0;
    for i in 0..ELS {
        if sel[i] == Logic::One {
            acc |= d[i].as_usize();
        }
    }
    data_o.write(Bits::from_usize(acc));
}

fn main() {
    let mut exec = HardwareExecutor::new();

    let (data_drv, data_in) = wire::<[Bits<WIDTH>; ELS], ()>([Bits::zero(); ELS]);
    let (sel_drv, sel_in) = wire::<Bits<ELS>, ()>(Bits::zero());
    let (out_drv, out_obs) = wire::<Bits<WIDTH>, ()>(Bits::zero());
    let dh = out_drv.dirty_handle();
    let reads = vec![data_in.wire_id(), sel_in.wire_id()];
    exec.spawn_wired(bsg_mux_one_hot(data_in, sel_in, out_drv), vec![dh], reads);

    let mut test = HardwareTest::new("bsg_mux_one_hot")
        .with_verilog("examples/basejump/sv/bsg_mux_one_hot.sv")
        .with_waveform("waveforms/bsg_mux_one_hot.vcd");

    // Coverage: BaseJump's testbench only sweeps one-hot selects (001,010,100).
    // The DUT is defined for ANY select (mask-then-OR), so we drive all 2^ELS
    // select patterns — including the zero and multi-hot cases — across many
    // randomized data rounds. The one-hot patterns (BaseJump's set) are a subset.
    // Golden = OR over i of (data_i[i] when sel_one_hot_i[i]).
    const N_ROUNDS: usize = 24;
    let mask = (1usize << WIDTH) - 1;
    let mut rng: u32 = 0xfeed_face;
    let mut cyc = 0usize;
    let mut expected_cycles = Vec::new();
    for _ in 0..N_ROUNDS {
        let mut data = [Bits::<WIDTH>::zero(); ELS];
        for slot in data.iter_mut() {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            *slot = Bits::from_usize(((rng >> 5) as usize) & mask);
        }
        let data_flat: Vec<Logic> = data
            .iter()
            .flat_map(|b| b.as_array().iter().copied())
            .collect();

        for sel_val in 0..(1usize << ELS) {
            let sel = Bits::<ELS>::from_usize(sel_val);
            let mut gold: usize = 0;
            for (e, slot) in data.iter().enumerate() {
                if (sel_val >> e) & 1 == 1 {
                    gold |= slot.as_usize();
                }
            }
            data_drv.write(data);
            sel_drv.write(sel);
            exec.poll_tasks();

            let out = out_obs.read();
            test.record_cycle(
                cyc,
                &[("data_i", &data_flat), ("sel_one_hot_i", sel.as_array())],
                &[("data_o", out.as_array())],
            );
            expected_cycles.push(make_cycle(
                cyc,
                &[("data_i", &data_flat), ("sel_one_hot_i", sel.as_array())],
                &[("data_o", &Bits::<WIDTH>::from_usize(gold).as_array()[..])],
            ));
            cyc += 1;
        }
    }

    let expected = SimulationTrace::from_cycles(expected_cycles);
    test.finish_with_expected(&expected).assert_passed();
    println!("bsg_mux_one_hot: Copper sim ≡ BaseJump golden model ≡ BaseJump Verilog (Verilator) ✓");
}
