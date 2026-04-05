use copper_core::{Clock, ClockDomain, Logic};
use copper_sim::{emit, HardwareExecutor, HardwareTest};
use copper_macros::hardware;
use std::fs;
use std::path::Path;
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

#[hardware(function_typed)]
async fn registered_alu(
    clk: Clock<MainClk>,
    op: Arc<Mutex<u8>>,
    a: Arc<Mutex<u8>>,
    b: Arc<Mutex<u8>>,
) -> u8 {
    let mut reg: u8 = 0;

    loop {
        emit!(reg);
        clk.tick().await;

        let op_val = *op.lock().unwrap();
        let a_val  = *a.lock().unwrap();
        let b_val  = *b.lock().unwrap();
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
    println!("=== Combinational ALU ===");
    for (op, a, b) in [(0u8, 10u8, 3u8), (1, 10, 3), (2, 0b1100, 0b1010), (3, 0b1100, 0b1010)] {
        println!("op={} a={} b={} -> out={}", op, a, b, alu(op, a, b));
    }

    println!("\n=== Sequential ALU ===");
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let op = Arc::new(Mutex::new(0u8));
    let a  = Arc::new(Mutex::new(0u8));
    let b  = Arc::new(Mutex::new(0u8));
    let out = exec.spawn_function_typed(
        0u8,
        registered_alu(clk.clone(), Arc::clone(&op), Arc::clone(&a), Arc::clone(&b)),
    );

    let pattern = vec![
        (0u8, 1u8, 2u8),
        (1u8, 5u8, 3u8),
        (2u8, 0b1010u8, 0b1100u8),
        (3u8, 0b1010u8, 0b1100u8),
    ];

    // Generate Verilog from Rust source and verify against it
    let verilog = copper_codegen::module_verilog!(registered_alu);
    let verilog_path = "verilog/generated-verilog/registered_alu.v";
    if let Some(parent) = Path::new(verilog_path).parent() {
        fs::create_dir_all(parent).expect("failed to create Verilog output directory");
    }
    fs::write(verilog_path, &verilog).expect("failed to write Verilog");
    println!("=== Generated Verilog ===\n{}", verilog);

    let mut test = HardwareTest::new("registered_alu")
        .with_verilog(verilog_path)
        .with_waveform("waveforms/alu.vcd");

    for (op_val, a_val, b_val) in pattern.iter().copied() {
        *op.lock().unwrap() = op_val;
        *a.lock().unwrap()  = a_val;
        *b.lock().unwrap()  = b_val;

        exec.tick_clock(&mut clk);
        let out_val = *out.lock().unwrap();
        println!("cycle {} op={} a={} b={} out={}", clk.cycle(), op_val, a_val, b_val, out_val);

        let op_logic  = u2_to_logic_vec(op_val);
        let a_logic   = u8_to_logic_vec(a_val);
        let b_logic   = u8_to_logic_vec(b_val);
        let out_logic = u8_to_logic_vec(out_val);
        test.record_cycle(
            clk.cycle() as usize,
            &[("op", &op_logic), ("a", &a_logic), ("b", &b_logic)],
            &[("out", &out_logic)],
        );
    }

    test.finish().assert_passed();
}
