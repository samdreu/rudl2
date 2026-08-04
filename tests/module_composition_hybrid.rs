//! Module-composition tests: combinational logic as plain functions, sequential
//! modules spawned as a tracked hierarchy (`spawn_child` / `module_info`) and as
//! peers (`spawn_untracked`). Uses the current `In<T,D>` / `Out<T,D>` wire port
//! model.

use copper_core::port::{wire, In, Out};
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::{spawn_child, HardwareExecutor};

struct MainClk;
impl ClockDomain for MainClk {}

// Combinational logic is just plain Rust functions — no hardware wrapper needed.
fn add_one(x: u8) -> u8 {
    x.wrapping_add(1)
}

fn double(x: u8) -> u8 {
    x.wrapping_add(x)
}

fn affine_mix(x: u8) -> u8 {
    double(add_one(x))
}

// A registered pipeline stage: drives `output` with the previous cycle's value,
// then latches `add_one(input)` into its register.
#[hardware(sequential)]
async fn stage_add_one(clk: Clock<MainClk>, input: In<u8, MainClk>, output: Out<u8, MainClk>) {
    let mut reg = 0u8;
    loop {
        clk.tick().await;
        output.write(reg);
        reg = add_one(input.read());
    }
}

#[hardware(sequential)]
async fn stage_double(clk: Clock<MainClk>, input: In<u8, MainClk>, output: Out<u8, MainClk>) {
    let mut reg = 0u8;
    loop {
        clk.tick().await;
        output.write(reg);
        reg = double(input.read());
    }
}

// `step` is a compile-time constant (a module parameter in hardware), so it is a
// const generic rather than a runtime port — which also lets it be a `#[hardware]`
// module (the macro requires ports to be `Clock`/`In`/`Out`).
#[hardware(sequential)]
async fn counter_by<const STEP: u8>(clk: Clock<MainClk>, output: Out<u8, MainClk>) {
    let mut reg = 0u8;
    loop {
        clk.tick().await;
        output.write(reg);
        reg = reg.wrapping_add(STEP);
    }
}

#[test]
fn combinational_modules_are_plain_functions() {
    assert_eq!(add_one(0), 1);
    assert_eq!(double(7), 14);
    assert_eq!(affine_mix(3), 8);
    assert_eq!(affine_mix(255), 0);
}

#[test]
fn sequential_modules_spawn_with_spawn_child() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    // in_data → stage_add_one → mid → stage_double → out_data
    let (in_drv, in_port) = wire::<u8, MainClk>(0);
    let (mid_out, mid_in) = wire::<u8, MainClk>(0);
    let (out_drv, out_obs) = wire::<u8, MainClk>(0);

    let s1_reads = vec![in_port.wire_id()];
    spawn_child!(
        exec,
        "pipeline",
        "stage_add_one",
        stage_add_one(clk.clone(), in_port, mid_out),
        s1_reads
    );
    let s2_reads = vec![mid_in.wire_id()];
    spawn_child!(
        exec,
        "pipeline",
        "stage_double",
        stage_double(clk.clone(), mid_in, out_drv),
        s2_reads
    );

    // Two-stage registered pipeline: affine_mix has a 2-cycle latency.
    let inputs = [3u8, 7, 11, 1];
    let expected_outputs = [0u8, 0, 8, 16];

    for (input, expected) in inputs.iter().zip(expected_outputs.iter()) {
        in_drv.write(*input);
        exec.tick_clock(&mut clk);
        let observed = out_obs.read();
        assert_eq!(observed, *expected, "cycle {}", clk.cycle());
    }

    let pipeline = exec.module_info("pipeline").expect("pipeline metadata missing");
    assert_eq!(pipeline.children.len(), 2);
    assert!(pipeline.children.contains(&"stage_add_one".to_string()));
    assert!(pipeline.children.contains(&"stage_double".to_string()));

    let stage1 = exec
        .module_info("stage_add_one")
        .expect("stage_add_one metadata missing");
    assert_eq!(stage1.parent.as_deref(), Some("pipeline"));

    let stage2 = exec
        .module_info("stage_double")
        .expect("stage_double metadata missing");
    assert_eq!(stage2.parent.as_deref(), Some("pipeline"));
}

#[test]
fn peer_modules_continue_using_spawn_untracked() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (out1_drv, out1_obs) = wire::<u8, MainClk>(0);
    let (out2_drv, out2_obs) = wire::<u8, MainClk>(0);
    let (out3_drv, out3_obs) = wire::<u8, MainClk>(0);

    exec.spawn_untracked(counter_by::<1>(clk.clone(), out1_drv), vec![]);
    exec.spawn_untracked(counter_by::<2>(clk.clone(), out2_drv), vec![]);
    exec.spawn_untracked(counter_by::<5>(clk.clone(), out3_drv), vec![]);

    let expected = [(0u8, 0u8, 0u8), (1, 2, 5), (2, 4, 10), (3, 6, 15)];

    for (exp1, exp2, exp3) in expected {
        exec.tick_clock(&mut clk);
        assert_eq!(out1_obs.read(), exp1, "counter1 cycle {}", clk.cycle());
        assert_eq!(out2_obs.read(), exp2, "counter2 cycle {}", clk.cycle());
        assert_eq!(out3_obs.read(), exp3, "counter3 cycle {}", clk.cycle());
    }
}
