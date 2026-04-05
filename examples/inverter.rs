use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};
use copper_macros::hardware;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware]
fn inverter(bit: Bit) -> Bit {
    match bit.0 {
        Logic::Zero => Bit::ONE,
        Logic::One => Bit::ZERO,
        Logic::X => Bit::X,
    }
}

#[hardware]
async fn registered_inverter(
    clk: Clock<MainClk>,
    input: Arc<Mutex<Bit>>,
    output: Arc<Mutex<Bit>>,
) {
    let mut reg = Bit::ZERO;

    loop {
        output.lock().unwrap().clone_from(&reg);
        clk.tick().await;
        let input_val = *input.lock().unwrap();
        reg = inverter(input_val);
    }
}

fn main() {
    // ── Combinational inverter ──────────────────────────────────────────────

    let comb_verilog = copper_codegen::module_verilog!(inverter);
    println!("=== Generated Verilog (Combinational) ===\n{}", comb_verilog);

    let comb_path = "verilog/generated-verilog/inverter.v";
    if let Some(parent) = Path::new(comb_path).parent() {
        fs::create_dir_all(parent).expect("failed to create Verilog output directory");
    }
    fs::write(comb_path, &comb_verilog).expect("failed to write combinational Verilog");

    let input_pattern = [Logic::Zero, Logic::One, Logic::X];
    let mut comb_test = HardwareTest::new("inverter")
        .with_verilog(comb_path)
        .with_waveform("waveforms/inverter_comb.vcd");

    for (i, &input) in input_pattern.iter().enumerate() {
        let output = inverter(Bit(input));
        comb_test.record_cycle(i + 1, &[("bit", &[input])], &[("out", &[output.0])]);
    }

    let comb_expected = SimulationTrace::from_cycles(vec![
        make_cycle(1, &[("bit", &[Logic::Zero])], &[("out", &[Logic::One])]),
        make_cycle(2, &[("bit", &[Logic::One])],  &[("out", &[Logic::Zero])]),
        make_cycle(3, &[("bit", &[Logic::X])],    &[("out", &[Logic::X])]),
    ]);

    comb_test.finish_with_expected(&comb_expected).assert_passed();

    // ── Sequential (registered) inverter ───────────────────────────────────

    let seq_verilog = copper_codegen::module_verilog!(registered_inverter);
    println!("\n=== Generated Verilog (Sequential) ===\n{}", seq_verilog);

    let seq_path = "verilog/generated-verilog/inverter_seq.v";
    fs::write(seq_path, &seq_verilog).expect("failed to write sequential Verilog");

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let input = Arc::new(Mutex::new(Bit::ZERO));
    let output = Arc::new(Mutex::new(Bit::ZERO));
    exec.spawn(registered_inverter(clk.clone(), Arc::clone(&input), Arc::clone(&output)));

    let seq_pattern = [Logic::Zero, Logic::One, Logic::Zero, Logic::One, Logic::Zero];
    let mut seq_test = HardwareTest::new("registered_inverter")
        .with_verilog(seq_path)
        .with_waveform("waveforms/inverter_seq.vcd");

    for (cycle_count, &input_logic) in seq_pattern.iter().enumerate() {
        *input.lock().unwrap() = Bit(input_logic);
        exec.tick_clock(&mut clk);
        let output_val = *output.lock().unwrap();

        seq_test.record_cycle(
            cycle_count + 1,
            &[("input_sig", &[input_logic])],
            &[("out", &[output_val.0])],
        );
    }

    seq_test.finish().assert_passed();
}
