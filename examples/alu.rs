use copper_core::{Clock, ClockDomain, Logic};
use copper_sim::{HardwareExecutor, SimulationTrace, verify_with_verilator};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

fn alu(op: u8, a: u8, b: u8) -> u8 {
    match op & 0x3 {
        0 => a.wrapping_add(b),
        1 => a.wrapping_sub(b),
        2 => a & b,
        _ => a | b,
    }
}

#[hardware]
async fn registered_alu(
    clk: Clock<MainClk>,
    op: Arc<Mutex<u8>>,
    a: Arc<Mutex<u8>>,
    b: Arc<Mutex<u8>>,
    out: Arc<Mutex<u8>>,
) {
    let mut reg: u8 = 0;

    loop {
        *out.lock().unwrap() = reg;
        clk.tick().await;

        let op_val = *op.lock().unwrap();
        let a_val = *a.lock().unwrap();
        let b_val = *b.lock().unwrap();
        reg = alu(op_val, a_val, b_val);
    }
}

fn u8_to_logic_vec(val: u8) -> Vec<Logic> {
    (0..8)
        .map(|i| if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

fn u2_to_logic_vec(val: u8) -> Vec<Logic> {
    (0..2)
        .map(|i| if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

fn main() {
    println!("=== Combinational ALU Tests ===");
    let tests = vec![
        (0u8, 10u8, 3u8),
        (1u8, 10u8, 3u8),
        (2u8, 0b1100u8, 0b1010u8),
        (3u8, 0b1100u8, 0b1010u8),
    ];

    for (op, a, b) in tests.iter().copied() {
        let out = alu(op, a, b);
        println!("op={} a={} b={} -> out={}", op, a, b, out);
    }

    println!("\n=== Sequential ALU Tests (Rust Simulation) ===");
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let op = Arc::new(Mutex::new(0u8));
    let a = Arc::new(Mutex::new(0u8));
    let b = Arc::new(Mutex::new(0u8));
    let out = Arc::new(Mutex::new(0u8));

    exec.spawn(registered_alu(
        clk.clone(),
        Arc::clone(&op),
        Arc::clone(&a),
        Arc::clone(&b),
        Arc::clone(&out),
    ));

    let pattern = vec![
        (0u8, 1u8, 2u8),
        (1u8, 5u8, 3u8),
        (2u8, 0b1010u8, 0b1100u8),
        (3u8, 0b1010u8, 0b1100u8),
    ];

    let mut trace = SimulationTrace::new();

    for (op_val, a_val, b_val) in pattern.iter().copied() {
        *op.lock().unwrap() = op_val;
        *a.lock().unwrap() = a_val;
        *b.lock().unwrap() = b_val;

        exec.tick_clock(&mut clk);
        let out_val = *out.lock().unwrap();

        println!("cycle {} op={} a={} b={} out={}", clk.cycle(), op_val, a_val, b_val, out_val);

        trace.add_cycle(
            clk.cycle() as usize,
            vec![
                ("op".to_string(), u2_to_logic_vec(op_val)),
                ("a".to_string(), u8_to_logic_vec(a_val)),
                ("b".to_string(), u8_to_logic_vec(b_val)),
            ],
            vec![("out".to_string(), u8_to_logic_vec(out_val))],
        );
    }

    println!("\n=== Cross-Validating with Verilator ===");
    match verify_with_verilator("verilog/alu.v", "alu", &trace) {
        Ok(true) => println!("✓ Verilator verification PASSED! Rust and Verilog match!"),
        Ok(false) => println!("✗ Verilator verification FAILED!"),
        Err(e) => println!("⚠ Verilator verification error: {}", e),
    }
}
