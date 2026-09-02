mod common;

use common::verilator_available;
use copper_core::Logic;
use copper_sim::{SimulationTrace, verify_with_verilator};

fn u8_to_logic_vec(val: u8) -> Vec<Logic> {
    (0..8)
        .map(|i| if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

#[test]
fn hierarchical_verilog_pipeline_matches_expected_trace() {
    // Only a genuinely ABSENT verilator may skip; installed-but-broken panics inside
    // `verilator_available` (a private `--version` probe used to treat both alike).
    if !verilator_available() {
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
