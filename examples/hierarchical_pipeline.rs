use copper_core::{Clock, ClockDomain, Logic};
use copper_sim::{emit, HardwareExecutor, SimulationTrace, verify_with_verilator};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// Stage 1: Increment by 1
fn stage1_comb(input: u8) -> u8 {
    input.wrapping_add(1)
}

#[hardware(function_typed)]
async fn stage1(
    clk: Clock<MainClk>,
    input: Arc<Mutex<u8>>,
) -> u8 {
    let mut reg = 0u8;
    loop {
        emit!(reg);
        clk.tick().await;
        reg = stage1_comb(*input.lock().unwrap());
    }
}

// Stage 2: Double (add to self)
fn stage2_comb(input: u8) -> u8 {
    input.wrapping_add(input)
}

#[hardware(function_typed)]
async fn stage2(
    clk: Clock<MainClk>,
    input: Arc<Mutex<u8>>,
) -> u8 {
    let mut reg = 0u8;
    loop {
        emit!(reg);
        clk.tick().await;
        let val = *input.lock().unwrap();
        reg = stage2_comb(val);
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
    let wire_s1_s2 = exec.spawn_child_function_typed(
        "stage1",
        "pipeline",
        0u8,
        stage1(clk.clone(), Arc::clone(&in_data)),
    );
    let out_data = exec.spawn_child_function_typed(
        "stage2",
        "pipeline",
        0u8,
        stage2(clk.clone(), Arc::clone(&wire_s1_s2)),
    );

    let test_inputs = vec![5u8, 10, 15];
    let mut trace = SimulationTrace::new();

    println!("=== Hierarchical 2-Stage Pipeline (Rust Simulation) ===");
    for &input in test_inputs.iter() {
        *in_data.lock().unwrap() = input;
        exec.tick_clock(&mut clk);
        let output = *out_data.lock().unwrap();
        let wire_val = *wire_s1_s2.lock().unwrap();
        println!("cycle {} input={:3} wire={:3} output={:3}", 
                 clk.cycle(), input, wire_val, output);

        trace.add_cycle(
            clk.cycle() as usize,
            vec![("in_data".to_string(), u8_to_logic_vec(input))],
            vec![("out_data".to_string(), u8_to_logic_vec(output))],
        );
    }

    // Drain 2 more cycles
    for _ in 0..2 {
        *in_data.lock().unwrap() = 0;
        exec.tick_clock(&mut clk);
        let output = *out_data.lock().unwrap();
        let wire_val = *wire_s1_s2.lock().unwrap();
        println!("cycle {} wire={:3} output={:3}", clk.cycle(), wire_val, output);

        trace.add_cycle(
            clk.cycle() as usize,
            vec![("in_data".to_string(), u8_to_logic_vec(0))],
            vec![("out_data".to_string(), u8_to_logic_vec(output))],
        );
    }

    println!("\n=== Cross-Validating with Verilator ===");
    match verify_with_verilator("verilog/pipeline.v", "pipeline", &trace) {
        Ok(true) => println!("✓ Verilator verification PASSED! Rust and Verilog match!"),
        Ok(false) => println!("✗ Verilator verification FAILED!"),
        Err(e) => println!("⚠ Verilator verification error: {}", e),
    }
}
