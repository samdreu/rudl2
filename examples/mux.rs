use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};
use copper_macros::hardware;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware]
fn mux(select: Bit, input0: Bit, input1: Bit) -> Bit {
    match select.0 {
        Logic::Zero => input0,
        Logic::One => input1,
        Logic::X => Bit::X,
    }
}

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
        output.lock().unwrap().clone_from(&reg);
        clk.tick().await;
        let sel = *select.lock().unwrap();
        let in0 = *input0.lock().unwrap();
        let in1 = *input1.lock().unwrap();
        reg = mux(sel, in0, in1);
    }
}

fn main() {
    // ── Combinational mux ──────────────────────────────────────────────────

    // (select, input0, input1, expected_output)
    let comb_cases = vec![
        (Logic::Zero, Logic::Zero, Logic::Zero, Logic::Zero),
        (Logic::Zero, Logic::One,  Logic::Zero, Logic::One),
        (Logic::One,  Logic::One,  Logic::Zero, Logic::Zero),
        (Logic::One,  Logic::One,  Logic::One,  Logic::One),
        (Logic::Zero, Logic::Zero, Logic::One,  Logic::Zero),
        (Logic::Zero, Logic::One,  Logic::One,  Logic::One),
    ];

    // Generate Verilog from Rust source
    let mux_verilog = copper_codegen::module_verilog!(mux);
    let mux_verilog_path = "verilog/generated-verilog/mux.v";
    if let Some(parent) = Path::new(mux_verilog_path).parent() {
        fs::create_dir_all(parent).expect("failed to create Verilog output directory");
    }
    fs::write(mux_verilog_path, &mux_verilog).expect("failed to write Verilog");
    println!("=== Generated Verilog (Combinational) ===\n{}", mux_verilog);

    let mut comb_test = HardwareTest::new("mux")
        .with_verilog(mux_verilog_path)
        .with_waveform("waveforms/mux_comb.vcd");

    let mut comb_expected_cycles = Vec::new();

    for (i, &(sel, in0, in1, exp)) in comb_cases.iter().enumerate() {
        let output = mux(Bit(sel), Bit(in0), Bit(in1));
        comb_test.record_cycle(
            i + 1,
            &[("select", &[sel]), ("input0", &[in0]), ("input1", &[in1])],
            &[("out", &[output.0])],
        );
        comb_expected_cycles.push(make_cycle(
            i + 1,
            &[("select", &[sel]), ("input0", &[in0]), ("input1", &[in1])],
            &[("out", &[exp])],
        ));
    }

    let comb_expected = SimulationTrace::from_cycles(comb_expected_cycles);
    comb_test.finish_with_expected(&comb_expected).assert_passed();

    // ── Sequential (registered) mux ────────────────────────────────────────

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

    // (select, input0, input1)
    let seq_pattern = vec![
        (Logic::Zero, Logic::Zero, Logic::Zero),
        (Logic::Zero, Logic::One,  Logic::Zero),
        (Logic::One,  Logic::One,  Logic::Zero),
        (Logic::One,  Logic::One,  Logic::One),
        (Logic::Zero, Logic::Zero, Logic::One),
    ];

    let reg_mux_verilog = copper_codegen::module_verilog!(registered_mux);
    let reg_mux_verilog_path = "verilog/generated-verilog/registered_mux.v";
    fs::write(reg_mux_verilog_path, &reg_mux_verilog).expect("failed to write Verilog");
    println!("=== Generated Verilog (Sequential) ===\n{}", reg_mux_verilog);

    let mut seq_test = HardwareTest::new("registered_mux")
        .with_verilog(reg_mux_verilog_path)
        .with_waveform("waveforms/mux_seq.vcd");

    for &(sel, in0, in1) in seq_pattern.iter() {
        *select.lock().unwrap() = Bit(sel);
        *input0.lock().unwrap() = Bit(in0);
        *input1.lock().unwrap() = Bit(in1);

        exec.tick_clock(&mut clk);
        let output_val = *output.lock().unwrap();

        if clk.cycle() > 1 {
            seq_test.record_cycle(
                (clk.cycle() - 1) as usize,
                &[("select", &[sel]), ("input0", &[in0]), ("input1", &[in1])],
                &[("out", &[output_val.0])],
            );
        }
    }

    seq_test.finish().assert_passed();
}
