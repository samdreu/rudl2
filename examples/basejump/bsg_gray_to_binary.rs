// Copper re-implementation of BaseJump STL `bsg_gray_to_binary`, checked for
// equivalence against the original BaseJump Verilog (+ its bsg_scan dependency)
// under Verilator (examples/basejump/sv/bsg_gray_to_binary.sv). Converts a Gray
// code to binary via a prefix-XOR: binary[k] = XOR of gray bits from k up to MSB.
//
// Parameters and stimulus follow BaseJump's own testbench
// testing/bsg_misc/bsg_gray_to_binary/test_bsg.sv: width_p=8, and the input is
// swept as gray = count ^ (count >> 1) for count = 0 .. 2^width-1. Golden:
// binary_o == count.

use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Logic};
use copper_macros::hardware;
use copper_sim::{make_cycle, HardwareExecutor, HardwareTest, SimulationTrace};

const WIDTH: usize = 8;

// binary_o[width-1] = gray_i[width-1]; binary_o[k] = gray_i[k] ^ binary_o[k+1].
#[hardware(combinational)]
fn bsg_gray_to_binary(gray_i: In<Bits<WIDTH>, ()>, binary_o: Out<Bits<WIDTH>, ()>) {
    let g = gray_i.read();
    let mut b = [Logic::Zero; WIDTH];
    b[WIDTH - 1] = g[WIDTH - 1];
    // A descending `for`, not a `while`: a combinational loop has to be fully
    // unrolled to be hardware, so its trip count must be a compile-time constant.
    // `for` says that; `while k > 0 { k -= 1; … }` only implies it. Same
    // recurrence, same order — b[k] = g[k] ^ b[k+1] for k from WIDTH-2 down to 0.
    for i in 1..WIDTH {
        b[WIDTH - 1 - i] = g[WIDTH - 1 - i] ^ b[WIDTH - i];
    }
    binary_o.write(Bits::from_slice(&b));
}

fn main() {
    let mut exec = HardwareExecutor::new();

    let (g_drv, g_in) = wire::<Bits<WIDTH>, ()>(Bits::zero());
    let (b_drv, b_obs) = wire::<Bits<WIDTH>, ()>(Bits::zero());
    let dh = b_drv.dirty_handle();
    let reads = vec![g_in.wire_id()];
    exec.spawn_wired(bsg_gray_to_binary(g_in, b_drv), vec![dh], reads);

    let mut test = HardwareTest::new("bsg_gray_to_binary")
        .with_verilog("examples/basejump/sv/bsg_gray_to_binary.sv")
        .with_waveform("waveforms/bsg_gray_to_binary.vcd");

    let mut expected_cycles = Vec::new();
    for count in 0..(1usize << WIDTH) {
        let gray = count ^ (count >> 1);
        let g = Bits::<WIDTH>::from_usize(gray);
        g_drv.write(g);
        exec.poll_tasks();

        let out = b_obs.read();
        test.record_cycle(
            count,
            &[("gray_i", g.as_array())],
            &[("binary_o", out.as_array())],
        );
        // Golden: binary_o == count.
        expected_cycles.push(make_cycle(
            count,
            &[("gray_i", g.as_array())],
            &[("binary_o", &Bits::<WIDTH>::from_usize(count).as_array()[..])],
        ));
    }

    let expected = SimulationTrace::from_cycles(expected_cycles);
    test.finish_with_expected(&expected).assert_passed();
    println!("bsg_gray_to_binary: Copper sim ≡ golden (binary == count) ≡ BaseJump Verilog (Verilator) ✓");
}
