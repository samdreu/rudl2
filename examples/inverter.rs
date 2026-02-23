use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{HardwareExecutor, SimulationTrace};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// Combinational inverter (pure function)
fn inverter(bit: Bit) -> Bit {
    match bit.0 {
        Logic::Zero => Bit::ONE,
        Logic::One => Bit::ZERO,
        Logic::X => Bit::X,
    }
}

// Sequential inverter with registered output
#[hardware]
async fn registered_inverter(
    clk: Clock<MainClk>,
    input: Arc<Mutex<Bit>>,
    output: Arc<Mutex<Bit>>,
) {
    let mut reg = Bit::ZERO;
    
    loop {
        // Output the registered value
        output.lock().unwrap().clone_from(&reg);
        
        clk.tick().await;
        
        // Capture input and compute for next cycle
        let input_val = *input.lock().unwrap();
        reg = inverter(input_val);
    }
}

fn main() {
    println!("=== Combinational Inverter Tests ===");
    
    // Test combinational inverter
    let test_cases = vec![
        (Bit::ZERO, Bit::ONE),
        (Bit::ONE, Bit::ZERO),
        (Bit::X, Bit::X),
    ];
    
    for (input, expected) in test_cases {
        let output = inverter(input);
        let status = if output == expected { "✓" } else { "✗" };
        println!("inverter({:?}) = {:?} (expected {:?}) {}", input.0, output.0, expected.0, status);
    }
    
    println!("\n=== Sequential Inverter Tests (Rust Simulation) ===");
    
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    
    let input = Arc::new(Mutex::new(Bit::ZERO));
    let output = Arc::new(Mutex::new(Bit::ZERO));
    
    exec.spawn(registered_inverter(clk.clone(), Arc::clone(&input), Arc::clone(&output)));
    
    let input_pattern = vec![Logic::Zero, Logic::One, Logic::Zero, Logic::One, Logic::Zero];
    let mut trace = SimulationTrace::new();
    let mut cycle_count = 0;
    
    for (idx, &input_logic) in input_pattern.iter().enumerate() {
        *input.lock().unwrap() = Bit(input_logic);
        exec.tick_clock(&mut clk);
        let output_val = *output.lock().unwrap();
        
        println!("cycle {} input={:?} output={:?}", 
                 clk.cycle(), input_logic, output_val.0);
        
        // Add to trace for Verilator verification (skip first cycle due to reset/initialization)
        if clk.cycle() > 1 {
            trace.add_cycle(
                cycle_count + 1,
                vec![("in_data".to_string(), vec![input_logic])],
                vec![("out_data".to_string(), vec![output_val.0])],
            );
            cycle_count += 1;
        }
    }
    
    // Cross-validate with Verilator
    println!("\n=== Cross-Validating with Verilator ===");
    match copper_sim::verify_with_verilator("verilog/inverter.v", "inverter", &trace) {
        Ok(true) => println!("✓ Verilator verification PASSED! Rust and Verilog match!"),
        Ok(false) => {
            println!("✗ Verilator verification FAILED!");
            std::process::exit(1);
        },
        Err(e) => {
            println!("⚠ Verilator verification error: {}", e);
            println!("   (Verilator may not be installed)");
        }
    }
}
