// Copper re-implementation of BaseJump STL `bsg_adder_one_hot`, checked for
// equivalence against the original BaseJump Verilog under Verilator
// (examples/basejump/sv/bsg_adder_one_hot.sv). Both inputs are one-hot; the output
// is one-hot at the sum of the two input indices.
//
// Parameters and stimulus follow BaseJump's own testbench
// testing/bsg_misc/bsg_adder_one_hot/test_bsg.sv: width_p=4, output_width_p=7, and
// a_i/b_i swept as (1 << ctr[1:0]) / (1 << ctr[3:2]) over ctr = 0..15, i.e. every
// pair of one-hot inputs. Golden: o == 1 << (index(a) + index(b)).

use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Logic};
use copper_macros::hardware;
use copper_sim::{make_cycle, HardwareExecutor, HardwareTest, SimulationTrace};

const WIDTH: usize = 4;
const OUT_W: usize = 7; // 2*WIDTH - 1

// o[ia+ib] = a_i[ia] & b_i[ib], OR-reduced per output bit (one-hot in, one-hot out).
#[hardware(combinational)]
fn bsg_adder_one_hot(
    a_i: In<Bits<WIDTH>, ()>,
    b_i: In<Bits<WIDTH>, ()>,
    o: Out<Bits<OUT_W>, ()>,
) {
    let a = a_i.read();
    let b = b_i.read();
    let mut out = [Logic::Zero; OUT_W];
    for ia in 0..WIDTH {
        for ib in 0..WIDTH {
            if a[ia] == Logic::One && b[ib] == Logic::One {
                out[ia + ib] = Logic::One; // ia+ib in 0..=6
            }
        }
    }
    o.write(Bits::from_slice(&out));
}

fn one_hot<const N: usize>(idx: usize) -> Bits<N> {
    let mut a = [Logic::Zero; N];
    a[idx] = Logic::One;
    Bits::from_slice(&a)
}

fn main() {
    let mut exec = HardwareExecutor::new();

    let (a_drv, a_in) = wire::<Bits<WIDTH>, ()>(one_hot(0));
    let (b_drv, b_in) = wire::<Bits<WIDTH>, ()>(one_hot(0));
    let (o_drv, o_obs) = wire::<Bits<OUT_W>, ()>(Bits::zero());
    let dh = o_drv.dirty_handle();
    let reads = vec![a_in.wire_id(), b_in.wire_id()];
    exec.spawn_wired(bsg_adder_one_hot(a_in, b_in, o_drv), vec![dh], reads);

    let mut test = HardwareTest::new("bsg_adder_one_hot")
        .with_verilog("examples/basejump/sv/bsg_adder_one_hot.sv")
        .with_waveform("waveforms/bsg_adder_one_hot.vcd");

    let mut expected_cycles = Vec::new();
    for ctr in 0..16usize {
        let ia = ctr & 0x3;
        let ib = (ctr >> 2) & 0x3;
        let a = one_hot::<WIDTH>(ia);
        let b = one_hot::<WIDTH>(ib);
        a_drv.write(a);
        b_drv.write(b);
        exec.poll_tasks();

        let out = o_obs.read();
        test.record_cycle(
            ctr,
            &[("a_i", a.as_array()), ("b_i", b.as_array())],
            &[("o", out.as_array())],
        );
        expected_cycles.push(make_cycle(
            ctr,
            &[("a_i", a.as_array()), ("b_i", b.as_array())],
            &[("o", one_hot::<OUT_W>(ia + ib).as_array())],
        ));
    }

    let expected = SimulationTrace::from_cycles(expected_cycles);
    test.finish_with_expected(&expected).assert_passed();
    println!("bsg_adder_one_hot: Copper sim ≡ golden (o = 1<<(ia+ib)) ≡ BaseJump Verilog (Verilator) ✓");
}
