use copper_core::port::*;
use copper_core::types::*;
use copper_sim::*;
use copper_macros::hardware;

#[hardware(combinational)]
fn one_bit_comparator(i0: In<Logic, ()>, i1: In<Logic, ()>, eq: Out<Logic, ()>) {
    let p0 = !i0.read() & !i1.read();
    let p1 = i0.read() & i1.read();
    eq.write(p0 | p1);
}


fn main() {
    let mut executor = HardwareExecutor::new();

    let (i0_drv, i0_in) = wire::<Logic, ()>(Logic::Zero);
    let (i1_drv, i1_in) = wire::<Logic, ()>(Logic::Zero);
    let (eq_out, eq_obs) = wire::<Logic, ()>(Logic::Zero);

    let dh = eq_out.dirty_handle();

    executor.spawn_wired(
        one_bit_comparator(i0_in, i1_in, eq_out),
        vec![dh],
    );

    let mut test = HardwareTest::new("one_bit_comparator")
        .with_verilog("/Users/sdreussi/project/final_copper/examples/combinational/sv/one_bit_comparator.sv")
        .with_waveform("waveforms/one_bit_comparator.vcd");

    // test cases
    let cases: &[(Logic, Logic, Logic)] = &[
    (Logic::Zero, Logic::Zero, Logic::One),  // i0, i1, expected eq
    (Logic::Zero, Logic::One, Logic::Zero),
    (Logic::One, Logic::Zero, Logic::Zero),
    (Logic::One, Logic::One, Logic::One),
    ];

    for (i, &(i0, i1, expected_eq)) in cases.iter().enumerate() {
        i0_drv.write(i0);
        i1_drv.write(i1);
        executor.poll_tasks();
        test.record_cycle(i,
            &[("i0", &[i0]), ("i1", &[i1])],
            &[("eq", &[eq_obs.read()])],
        );
    }

    let expected = SimulationTrace::from_cycles(
        cases.iter().enumerate().map(|(i, &(i0, i1, eq))| {
            make_cycle(i, &[("i0", &[i0]), ("i1", &[i1])], &[("eq", &[eq])])
        }).collect()
    );

    test.finish_with_expected(&expected).assert_passed();


}
