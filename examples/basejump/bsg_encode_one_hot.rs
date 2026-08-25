// Copper re-implementation of BaseJump STL `bsg_encode_one_hot`, checked for
// equivalence against the original BaseJump Verilog under Verilator
// (examples/basejump/sv/bsg_encode_one_hot.sv). Encodes a one-hot input into its
// binary index (addr_o) and reports whether any bit is set (v_o). For a zero
// input, addr_o = 0 and v_o = 0. Non-one-hot inputs are undefined (not exercised).
//
// Parameters and stimulus follow BaseJump's own testbench
// testing/bsg_misc/bsg_encode_one_hot/test_bsg.sv: the input is a single 1 shifted
// from bit 0 upward, plus the all-zero case. Here width_p=8 (addr_o is 3 bits).
// Golden: addr_o == index of the set bit, v_o == (input != 0).

use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Logic};
use copper_macros::hardware;
use copper_sim::{make_cycle, HardwareExecutor, HardwareTest, SimulationTrace};

const WIDTH: usize = 8;
const ADDR_W: usize = 3; // $clog2(8)

#[hardware(combinational)]
fn bsg_encode_one_hot(
    i: In<Bits<WIDTH>, ()>,
    addr_o: Out<Bits<ADDR_W>, ()>,
    v_o: Out<Logic, ()>,
) {
    let inp = i.read();
    // `Bits<ADDR_W>`, not a bare `usize`: a `usize` local is a 32-bit signal, so
    // driving the 3-bit `addr_o` from it is a width truncation Verilator rejects
    // under `-Wall`. The accumulator is an address; its type should say so.
    let mut addr = Bits::<ADDR_W>::zero();
    let mut valid = Logic::Zero;
    for k in 0..WIDTH {
        if inp[k] == Logic::One {
            addr = Bits::from_usize(k);
            valid = Logic::One;
        }
    }
    addr_o.write(addr);
    v_o.write(valid);
}

// `#[cfg(not(test))]` so `tests/` can `include!` this file for its own
// harness without pulling in a second `main` (same structure as sipo_block).
#[cfg(not(test))]
fn main() {
    let mut exec = HardwareExecutor::new();

    let (i_drv, i_in) = wire::<Bits<WIDTH>, ()>(Bits::zero());
    let (addr_drv, addr_obs) = wire::<Bits<ADDR_W>, ()>(Bits::zero());
    let (v_drv, v_obs) = wire::<Logic, ()>(Logic::Zero);
    let dh_a = addr_drv.dirty_handle();
    let dh_v = v_drv.dirty_handle();
    let reads = vec![i_in.wire_id()];
    exec.spawn_wired(bsg_encode_one_hot(i_in, addr_drv, v_drv), vec![dh_a, dh_v], reads);

    let mut test = HardwareTest::new("bsg_encode_one_hot")
        .with_verilog("examples/basejump/sv/bsg_encode_one_hot.sv")
        .with_waveform("waveforms/bsg_encode_one_hot.vcd");

    // one-hot 00000001 .. 10000000, then all-zero (v_o = 0).
    let mut inputs: Vec<usize> = (0..WIDTH).map(|k| 1usize << k).collect();
    inputs.push(0);

    let mut expected_cycles = Vec::new();
    for (cyc, &val) in inputs.iter().enumerate() {
        let iv = Bits::<WIDTH>::from_usize(val);
        i_drv.write(iv);
        exec.poll_tasks();

        let (exp_addr, exp_v) = if val == 0 {
            (0usize, Logic::Zero)
        } else {
            (val.trailing_zeros() as usize, Logic::One)
        };

        let addr = addr_obs.read();
        let v = v_obs.read();
        test.record_cycle(
            cyc,
            &[("i", iv.as_array())],
            &[("addr_o", addr.as_array()), ("v_o", std::slice::from_ref(&v))],
        );
        expected_cycles.push(make_cycle(
            cyc,
            &[("i", iv.as_array())],
            &[
                ("addr_o", &Bits::<ADDR_W>::from_usize(exp_addr).as_array()[..]),
                ("v_o", std::slice::from_ref(&exp_v)),
            ],
        ));
    }

    let expected = SimulationTrace::from_cycles(expected_cycles);
    test.finish_with_expected(&expected).assert_passed();
    println!("bsg_encode_one_hot: Copper sim ≡ golden (addr=index, v=any) ≡ BaseJump Verilog (Verilator) ✓");
}
