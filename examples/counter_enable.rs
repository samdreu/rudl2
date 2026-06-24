use copper_core::{Bits, port::In, port::Out, Clock, ClockDomain, Logic};
use copper_core::port::wire;
use copper_sim::{HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};

struct MainClk;
impl ClockDomain for MainClk {}

// Matches counter_enable.sv: on posedge clk, evaluate reset/load/up_down in priority order.
// Tick-first so the update runs exactly once per clock edge.
async fn counter_enable(
    clk: Clock<MainClk>,
    reset: In<Logic>,
    load: In<Logic>,
    up_down: In<Logic>,
    data: In<Bits<4>>,
    count: Out<Bits<4>>,
) {
    let mut amount: Bits<4> = Bits::from_lit::<0>();
    loop {
        count.write(amount);
        clk.tick().await;
        if reset.read().into() {
            amount = Bits::from_lit::<0>();
        } else if load.read().into() {
            amount = data.read();
        } else if up_down.read().into() {
            amount = amount + Bits::from_lit::<1>();
        } else {
            amount = amount - Bits::from_lit::<1>();
        }
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (reset_drv, reset_in)     = wire::<Logic, ()>(Logic::Zero);
    let (load_drv, load_in)       = wire::<Logic, ()>(Logic::Zero);
    let (up_down_drv, up_down_in) = wire::<Logic, ()>(Logic::Zero);
    let (data_drv, data_in)       = wire::<Bits<4>, ()>(Bits::from_lit::<0>());
    let (count_out, count_obs)    = wire::<Bits<4>, ()>(Bits::from_lit::<0>());

    let dh = count_out.dirty_handle();
    exec.spawn_wired(
        counter_enable(clk.clone(), reset_in, load_in, up_down_in, data_in, count_out),
        vec![dh],
    );

    // The SV module name is "counter" — must match the module declaration in counter_enable.sv.
    let mut test = HardwareTest::new("counter")
        .with_verilog("examples/counter_enable.sv")
        .with_waveform("waveforms/counter_enable.vcd");

    let z = Logic::Zero;
    let o = Logic::One;

    // Cycles 1-3: count up
    up_down_drv.write(o);
    exec.tick_clock(&mut clk);
    test.record_cycle(0, &[("reset", &[z]), ("load", &[z]), ("up_down", &[o]), ("data", Bits::<4>::from_lit::<0>().as_array())], &[("count", count_obs.read().as_array())]);

    exec.tick_clock(&mut clk);
    test.record_cycle(1, &[("reset", &[z]), ("load", &[z]), ("up_down", &[o]), ("data", Bits::<4>::from_lit::<0>().as_array())], &[("count", count_obs.read().as_array())]);

    exec.tick_clock(&mut clk);
    test.record_cycle(2, &[("reset", &[z]), ("load", &[z]), ("up_down", &[o]), ("data", Bits::<4>::from_lit::<0>().as_array())], &[("count", count_obs.read().as_array())]);

    // Cycle 4: load overrides up_down
    up_down_drv.write(z);
    load_drv.write(o);
    data_drv.write(Bits::from_lit::<7>());
    exec.tick_clock(&mut clk);
    test.record_cycle(3, &[("reset", &[z]), ("load", &[o]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<7>().as_array())], &[("count", count_obs.read().as_array())]);

    // Cycle 5: count down
    load_drv.write(z);
    exec.tick_clock(&mut clk);
    test.record_cycle(4, &[("reset", &[z]), ("load", &[z]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<7>().as_array())], &[("count", count_obs.read().as_array())]);

    // Cycle 6: reset overrides load
    reset_drv.write(o);
    load_drv.write(o);
    data_drv.write(Bits::from_lit::<15>());
    exec.tick_clock(&mut clk);
    test.record_cycle(5, &[("reset", &[o]), ("load", &[o]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<15>().as_array())], &[("count", count_obs.read().as_array())]);

    // Cycle 7: release reset, count up
    reset_drv.write(z);
    load_drv.write(z);
    up_down_drv.write(o);
    exec.tick_clock(&mut clk);
    test.record_cycle(6, &[("reset", &[z]), ("load", &[z]), ("up_down", &[o]), ("data", Bits::<4>::from_lit::<15>().as_array())], &[("count", count_obs.read().as_array())]);

    // Cycle 8: load 0
    up_down_drv.write(z);
    load_drv.write(o);
    data_drv.write(Bits::from_lit::<0>());
    exec.tick_clock(&mut clk);
    test.record_cycle(7, &[("reset", &[z]), ("load", &[o]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<0>().as_array())], &[("count", count_obs.read().as_array())]);

    // Cycle 9: count down from 0 (wraps to 15)
    load_drv.write(z);
    exec.tick_clock(&mut clk);
    test.record_cycle(8, &[("reset", &[z]), ("load", &[z]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<0>().as_array())], &[("count", count_obs.read().as_array())]);

    // Expected trace — matches SV always@(posedge clk) behavior
    let expected = SimulationTrace::from_cycles(vec![
        make_cycle(0, &[("reset", &[z]), ("load", &[z]), ("up_down", &[o]), ("data", Bits::<4>::from_lit::<0>().as_array())],  &[("count", Bits::<4>::from_lit::<1>().as_array())]),
        make_cycle(1, &[("reset", &[z]), ("load", &[z]), ("up_down", &[o]), ("data", Bits::<4>::from_lit::<0>().as_array())],  &[("count", Bits::<4>::from_lit::<2>().as_array())]),
        make_cycle(2, &[("reset", &[z]), ("load", &[z]), ("up_down", &[o]), ("data", Bits::<4>::from_lit::<0>().as_array())],  &[("count", Bits::<4>::from_lit::<3>().as_array())]),
        make_cycle(3, &[("reset", &[z]), ("load", &[o]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<7>().as_array())],  &[("count", Bits::<4>::from_lit::<7>().as_array())]),
        make_cycle(4, &[("reset", &[z]), ("load", &[z]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<7>().as_array())],  &[("count", Bits::<4>::from_lit::<6>().as_array())]),
        make_cycle(5, &[("reset", &[o]), ("load", &[o]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<15>().as_array())], &[("count", Bits::<4>::from_lit::<0>().as_array())]),
        make_cycle(6, &[("reset", &[z]), ("load", &[z]), ("up_down", &[o]), ("data", Bits::<4>::from_lit::<15>().as_array())], &[("count", Bits::<4>::from_lit::<1>().as_array())]),
        make_cycle(7, &[("reset", &[z]), ("load", &[o]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<0>().as_array())],  &[("count", Bits::<4>::from_lit::<0>().as_array())]),
        make_cycle(8, &[("reset", &[z]), ("load", &[z]), ("up_down", &[z]), ("data", Bits::<4>::from_lit::<0>().as_array())],  &[("count", Bits::<4>::from_lit::<15>().as_array())]),
    ]);

    test.finish_with_expected(&expected).assert_passed();
}
