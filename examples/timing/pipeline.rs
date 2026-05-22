use copper_core::{Clock, ClockDomain, Logic};
use copper_sim::{emit, HardwareExecutor, HardwareTest};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// 2-stage pipeline: stage1 increments by 1, stage2 doubles (adds to itself)
#[hardware(function_typed)]
async fn registered_pipeline(
    clk: Clock<MainClk>,
    in_data: Arc<Mutex<u8>>,
) -> u8 {
    let mut stage1_data_r: u8 = 0;
    let mut stage2_data_r: u8 = 0;

    loop {
        emit!(stage2_data_r);
        clk.tick().await;

        let input_val = *in_data.lock().unwrap();
        let stage1_data = input_val.wrapping_add(1);
        let stage2_data = stage1_data_r.wrapping_add(stage1_data_r);

        stage1_data_r = stage1_data;
        stage2_data_r = stage2_data;
    }
}

fn u8_to_logic_vec(val: u8) -> Vec<Logic> {
    (0..8)
        .map(|i| if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let in_data = Arc::new(Mutex::new(0u8));
    let out_data = exec.spawn_function_typed(
        0u8,
        registered_pipeline(clk.clone(), Arc::clone(&in_data)),
    );

    let test_inputs = vec![5u8, 10, 15];

    let mut test = HardwareTest::new("pipeline")
        .with_verilog("verilog/pipeline.v")
        .with_waveform("waveforms/pipeline.vcd");

    println!("=== 2-Stage Pipeline ===");
    for &input in test_inputs.iter() {
        *in_data.lock().unwrap() = input;
        exec.tick_clock(&mut clk);
        let output = *out_data.lock().unwrap();
        println!("cycle {} input={:3} output={:3}", clk.cycle(), input, output);

        let in_logic  = u8_to_logic_vec(input);
        let out_logic = u8_to_logic_vec(output);
        test.record_cycle(
            clk.cycle() as usize,
            &[("in_data",  &in_logic)],
            &[("out_data", &out_logic)],
        );
    }

    // Drain 2 more cycles to flush the pipeline
    for _ in 0..2 {
        *in_data.lock().unwrap() = 0;
        exec.tick_clock(&mut clk);
        let output = *out_data.lock().unwrap();
        println!("cycle {} output={:3}", clk.cycle(), output);

        let in_logic  = u8_to_logic_vec(0);
        let out_logic = u8_to_logic_vec(output);
        test.record_cycle(
            clk.cycle() as usize,
            &[("in_data",  &in_logic)],
            &[("out_data", &out_logic)],
        );
    }

    test.finish().assert_passed();
}
