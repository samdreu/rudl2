use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{HardwareExecutor, SimulationTrace};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// Combinational mux (pure function)
fn mux(select: Bit, input0: Bit, input1: Bit) -> Bit {
    match select.0 {
        Logic::Zero => input0,
        Logic::One => input1,
        Logic::X => Bit::X,
    }
}

// Sequential mux with registered output
#[hardware]
async fn registered_mux(
    clk: Clock<MainClk>,
    select: Arc<Mutex<Bit>>,
    input0: Arc<Mutex<Bit>>,
    input1: Arc<Mutex<Bit>>,
    output: Arc<Mutex<Bit>>,
) {
    let mut reg = Bit::ZERO;
    
    loop {
        // Output the registered value
        output.lock().unwrap().clone_from(&reg);
        
        clk.tick().await;
        
        // Capture inputs and compute for next cycle
        let sel = *select.lock().unwrap();
        let in0 = *input0.lock().unwrap();
        let in1 = *input1.lock().unwrap();
        reg = mux(sel, in0, in1);
    }
}

fn main() {
    println!("=== Combinational Mux Tests ===");
    
    // Test combinational mux
    let test_cases = vec![
        // (select, input0, input1, expected_output)
        (Bit::ZERO, Bit::ZERO, Bit::ZERO, Bit::ZERO),
        (Bit::ZERO, Bit::ONE, Bit::ZERO, Bit::ONE),
        (Bit::ONE, Bit::ZERO, Bit::ZERO, Bit::ZERO),
        (Bit::ONE, Bit::ONE, Bit::ZERO, Bit::ZERO),
        (Bit::ONE, Bit::ZERO, Bit::ONE, Bit::ONE),
        (Bit::ZERO, Bit::ONE, Bit::ONE, Bit::ONE),
    ];
    
    for (select, in0, in1, expected) in test_cases {
        let output = mux(select, in0, in1);
        let status = if output == expected { "✓" } else { "✗" };
        println!("mux(sel={:?}, in0={:?}, in1={:?}) = {:?} (expected {:?}) {}", 
                 select.0, in0.0, in1.0, output.0, expected.0, status);
    }
    
    println!("\n=== Sequential Mux Tests (Rust Simulation) ===");
    
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    
    let select = Arc::new(Mutex::new(Bit::ZERO));
    let input0 = Arc::new(Mutex::new(Bit::ZERO));
    let input1 = Arc::new(Mutex::new(Bit::ZERO));
    let output = Arc::new(Mutex::new(Bit::ZERO));
    
    exec.spawn(registered_mux(
        clk.clone(),
        Arc::clone(&select),
        Arc::clone(&input0),
        Arc::clone(&input1),
        Arc::clone(&output),
    ));
    
    // Run simulation and build trace
    // Test pattern: select different inputs across cycles
    let test_pattern = vec![
        // (select, input0, input1)
        (Logic::Zero, Logic::Zero, Logic::Zero),   // sel=0 -> in0
        (Logic::Zero, Logic::One, Logic::Zero),    // sel=0 -> in0
        (Logic::One, Logic::One, Logic::Zero),     // sel=1 -> in1
        (Logic::One, Logic::One, Logic::One),      // sel=1 -> in1
        (Logic::Zero, Logic::Zero, Logic::One),    // sel=0 -> in0
    ];
    
    let mut trace = SimulationTrace::new();
    
    for (idx, &(sel, in0, in1)) in test_pattern.iter().enumerate() {
        *select.lock().unwrap() = Bit(sel);
        *input0.lock().unwrap() = Bit(in0);
        *input1.lock().unwrap() = Bit(in1);
        
        exec.tick_clock(&mut clk);
        let output_val = *output.lock().unwrap();
        
        println!("cycle {} sel={:?} in0={:?} in1={:?} output={:?}", 
                 clk.cycle(), sel, in0, in1, output_val.0);
        
        // Add to trace for Verilator verification (skip first cycle)
        if clk.cycle() > 1 {
            trace.add_cycle(
                (clk.cycle() - 1) as usize,
                vec![
                    ("select".to_string(), vec![sel]),
                    ("input0".to_string(), vec![in0]),
                    ("input1".to_string(), vec![in1]),
                ],
                vec![("out_data".to_string(), vec![output_val.0])],
            );
        }
    }
    
    // Cross-validate with Verilator
    println!("\n=== Cross-Validating with Verilator ===");
    match copper_sim::verify_with_verilator("verilog/mux.v", "mux", &trace) {
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
