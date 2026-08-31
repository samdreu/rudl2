use copper_core::Logic;
use copper_sim::{SimulationTrace, verify_with_verilator};
use std::process::Command;

fn u8_to_logic_vec(val: u8) -> Vec<Logic> {
    (0..8)
        .map(|i| if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

fn has_verilator() -> bool {
    Command::new("verilator")
        .arg("--version")
    .env_remove("VERILATOR_ROOT")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[test]
fn hierarchical_verilog_pipeline_matches_expected_trace() {
    if !has_verilator() {
        eprintln!("Skipping Verilator test: 'verilator' not found in PATH");
        return;
    }

    let inputs = [3u8, 7, 11, 1];
    let expected_outputs = [0u8, 0, 8, 16];

    let mut trace = SimulationTrace::new();
    for (cycle_idx, (input, expected)) in inputs.iter().zip(expected_outputs.iter()).enumerate() {
        trace.add_cycle(
            cycle_idx + 1,
            vec![("in_data".to_string(), u8_to_logic_vec(*input))],
            vec![("out_data".to_string(), u8_to_logic_vec(*expected))],
        );
    }

    let verified = verify_with_verilator("tests/fixtures/reference_sv/hybrid_pipeline.sv", "hybrid_pipeline", &trace)
        .expect("Verilator verification should run successfully");
    assert!(verified, "Expected Verilator comparison to pass");
}
