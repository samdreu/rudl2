use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{HardwareExecutor, SimulationTrace, verify_with_verilator};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// 2-stage pipeline with ready/valid
#[hardware]
async fn pipeline_ready_valid(
    clk: Clock<MainClk>,
    in_valid: Arc<Mutex<Bit>>,
    in_data: Arc<Mutex<u8>>,
    out_ready: Arc<Mutex<Bit>>,
    in_ready: Arc<Mutex<Bit>>,
    out_valid: Arc<Mutex<Bit>>,
    out_data: Arc<Mutex<u8>>,
) {
    let mut s1_valid = Bit::ZERO;
    let mut s1_data: u8 = 0;
    let mut s2_valid = Bit::ZERO;
    let mut s2_data: u8 = 0;

    loop {
        // Output registered state
        in_ready.lock().unwrap().clone_from(&Bit::from_bool(s1_valid == Bit::ZERO || s2_valid == Bit::ZERO || out_ready.lock().unwrap().0 == Logic::One));
        out_valid.lock().unwrap().clone_from(&s2_valid);
        *out_data.lock().unwrap() = s2_data;

        clk.tick().await;

        let in_v = *in_valid.lock().unwrap();
        let in_d = *in_data.lock().unwrap();
        let out_r = *out_ready.lock().unwrap();

        let s2_accept = s2_valid == Bit::ZERO || out_r == Bit::ONE;
        let s1_accept = s1_valid == Bit::ZERO || s2_accept;

        // Update stage2
        if s2_accept {
            if s1_valid == Bit::ONE {
                s2_valid = Bit::ONE;
                s2_data = s1_data.wrapping_add(s1_data);
            } else {
                s2_valid = Bit::ZERO;
            }
        }

        // Update stage1
        if s1_accept {
            if in_v == Bit::ONE {
                s1_valid = Bit::ONE;
                s1_data = in_d.wrapping_add(1);
            } else {
                s1_valid = Bit::ZERO;
            }
        }
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

    let in_valid = Arc::new(Mutex::new(Bit::ZERO));
    let in_data = Arc::new(Mutex::new(0u8));
    let out_ready = Arc::new(Mutex::new(Bit::ONE));
    let in_ready = Arc::new(Mutex::new(Bit::ONE));
    let out_valid = Arc::new(Mutex::new(Bit::ZERO));
    let out_data = Arc::new(Mutex::new(0u8));

    exec.spawn(pipeline_ready_valid(
        clk.clone(),
        Arc::clone(&in_valid),
        Arc::clone(&in_data),
        Arc::clone(&out_ready),
        Arc::clone(&in_ready),
        Arc::clone(&out_valid),
        Arc::clone(&out_data),
    ));

    let inputs = vec![1u8, 2, 3, 4, 5];
    let ready_pattern = vec![Logic::One, Logic::Zero, Logic::One, Logic::One, Logic::Zero, Logic::One, Logic::One];

    let mut trace = SimulationTrace::new();
    let mut in_idx = 0usize;

    println!("=== Pipeline Ready/Valid Tests (Rust Simulation) ===");
    for &ready in ready_pattern.iter() {
        *out_ready.lock().unwrap() = Bit(ready);

        // Drive input if available and ready
        let can_send = *in_ready.lock().unwrap() == Bit::ONE && in_idx < inputs.len();
        if can_send {
            *in_valid.lock().unwrap() = Bit::ONE;
            *in_data.lock().unwrap() = inputs[in_idx];
            in_idx += 1;
        } else {
            *in_valid.lock().unwrap() = Bit::ZERO;
        }

        exec.tick_clock(&mut clk);

        let iv = *in_valid.lock().unwrap();
        let id = *in_data.lock().unwrap();
        let ir = *in_ready.lock().unwrap();
        let ov = *out_valid.lock().unwrap();
        let od = *out_data.lock().unwrap();

        println!(
            "cycle {} in_valid={:?} in_ready={:?} in_data={} out_valid={:?} out_ready={:?} out_data={}",
            clk.cycle(), iv.0, ir.0, id, ov.0, ready, od
        );

        trace.add_cycle(
            clk.cycle() as usize,
            vec![
                ("in_valid".to_string(), vec![iv.0]),
                ("in_data".to_string(), u8_to_logic_vec(id)),
                ("out_ready".to_string(), vec![ready]),
            ],
            vec![
                ("in_ready".to_string(), vec![ir.0]),
                ("out_valid".to_string(), vec![ov.0]),
                ("out_data".to_string(), u8_to_logic_vec(od)),
            ],
        );
    }

    println!("\n=== Cross-Validating with Verilator ===");
    match verify_with_verilator("verilog/pipeline_stall.v", "pipeline_stall", &trace) {
        Ok(true) => println!("✓ Verilator verification PASSED! Rust and Verilog match!"),
        Ok(false) => println!("✗ Verilator verification FAILED!"),
        Err(e) => println!("⚠ Verilator verification error: {}", e),
    }
}
