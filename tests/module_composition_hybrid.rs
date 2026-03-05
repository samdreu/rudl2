use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, spawn_child};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

fn add_one(x: u8) -> u8 {
    x.wrapping_add(1)
}

fn double(x: u8) -> u8 {
    x.wrapping_add(x)
}

fn affine_mix(x: u8) -> u8 {
    double(add_one(x))
}

#[hardware]
async fn stage_add_one(clk: Clock<MainClk>, input: Arc<Mutex<u8>>, output: Arc<Mutex<u8>>) {
    let mut reg = 0u8;
    loop {
        clk.tick().await;
        *output.lock().unwrap() = reg;
        let in_val = *input.lock().unwrap();
        reg = add_one(in_val);
    }
}

#[hardware]
async fn stage_double(clk: Clock<MainClk>, input: Arc<Mutex<u8>>, output: Arc<Mutex<u8>>) {
    let mut reg = 0u8;
    loop {
        clk.tick().await;
        *output.lock().unwrap() = reg;
        let in_val = *input.lock().unwrap();
        reg = double(in_val);
    }
}

#[hardware]
async fn counter_by(clk: Clock<MainClk>, step: u8, output: Arc<Mutex<u8>>) {
    let mut reg = 0u8;
    loop {
        clk.tick().await;
        *output.lock().unwrap() = reg;
        reg = reg.wrapping_add(step);
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

    let in_data = Arc::new(Mutex::new(0u8));
    let wire = Arc::new(Mutex::new(0u8));
    let out_data = Arc::new(Mutex::new(0u8));

    spawn_child!(
        exec,
        "pipeline",
        "stage_add_one",
        stage_add_one(clk.clone(), Arc::clone(&in_data), Arc::clone(&wire))
    );
    spawn_child!(
        exec,
        "pipeline",
        "stage_double",
        stage_double(clk.clone(), Arc::clone(&wire), Arc::clone(&out_data))
    );

    let inputs = [3u8, 7, 11, 1];
    let expected_outputs = [0u8, 0, 8, 16];

    for (input, expected) in inputs.iter().zip(expected_outputs.iter()) {
        *in_data.lock().unwrap() = *input;
        exec.tick_clock(&mut clk);
        let observed = *out_data.lock().unwrap();
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
fn peer_modules_continue_using_exec_spawn() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let out1 = Arc::new(Mutex::new(0u8));
    let out2 = Arc::new(Mutex::new(0u8));
    let out3 = Arc::new(Mutex::new(0u8));

    exec.spawn(counter_by(clk.clone(), 1, Arc::clone(&out1)));
    exec.spawn(counter_by(clk.clone(), 2, Arc::clone(&out2)));
    exec.spawn(counter_by(clk.clone(), 5, Arc::clone(&out3)));

    let expected = [(0u8, 0u8, 0u8), (1, 2, 5), (2, 4, 10), (3, 6, 15)];

    for (exp1, exp2, exp3) in expected {
        exec.tick_clock(&mut clk);
        assert_eq!(*out1.lock().unwrap(), exp1, "counter1 cycle {}", clk.cycle());
        assert_eq!(*out2.lock().unwrap(), exp2, "counter2 cycle {}", clk.cycle());
        assert_eq!(*out3.lock().unwrap(), exp3, "counter3 cycle {}", clk.cycle());
    }
}
