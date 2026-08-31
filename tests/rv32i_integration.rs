//! End-to-end integration tests for the RV32I CPUs (P5, `TODO` TESTING plan).
//!
//! The flagship examples had ZERO `cargo test` coverage — they only self-checked
//! inside their own `fn main` (run via `cargo run --example`). This lifts those
//! architectural self-checks into `cargo test` and adds a scalar-vs-pipelined
//! differential.
//!
//! Structure (per the agreed "cfg-guard main + include" approach): the scalar
//! example is `include!`d at the crate root and the pipelined one inside `mod pl`,
//! so their identically-named harnesses (`run_program`, `test_*`, `MainClk`, the
//! assemblers) don't collide. Each example's `fn main` is `#[cfg(not(test))]`, so
//! it is excluded here. The CPUs do not transpile (a `Vec` program port + `Memory`
//! + submodules), so this is a simulation self-check against known architectural
//! results — not a Verilator equivalence.
//!
//! `run_program(prog, max) -> (a0, cycles)` runs the CPU until it halts and returns
//! the final `a0` register. A wrong result is a real CPU/simulator bug.

#![allow(dead_code)] // each example brings a full harness; tests use a subset

// REPOINTED 2026-08-26: this used to include `examples/cpu/rv32i_cpu.rs`, which
// was moved to untracked `old/` during the subset-restriction refactor (its `Vec`
// ports are outside the admissible grammar and no longer compile). The scalar
// role is now played by `rv32i_cpu_transpilable.rs` — same design, same 13
// programs, identical cycle counts (its header records the equivalence), an
// identical `run_program(Vec<u32>, usize) -> (u32, usize)` harness, and it is
// tracked and sweep-covered. The `scalar_and_pipelined_agree` differential below
// is therefore transpilable-vs-pipelined now.
include!("../examples/cpu/rv32i_cpu_transpilable.rs");

mod pl {
    include!("../examples/cpu/rv32i_cpu_pipelined.rs");
}

/// Generous halt budget: every program below halts far sooner; a real hang trips
/// `run_program`'s own "did not halt" panic rather than looping forever.
const MAX_CYCLES: usize = 20_000;

/// The programs both CPUs implement, with their known architectural result in `a0`.
/// (Machine code is CPU-independent, so the scalar assemblers build the vectors for
/// both — the pipelined CPU is fed the exact same bytes.)
fn common_cases() -> Vec<(&'static str, Vec<u32>, u32)> {
    vec![
        ("addi", test_addi(), 15),
        ("sub", test_sub(), 5),
        ("multiple_adds", test_multiple_adds(), 15),
        ("branch_taken", test_branch_taken(), 42),
        ("branch_not_taken", test_branch_not_taken(), 99),
        ("load_store", test_load_store(), 88),
        ("negative_numbers", test_negative_numbers(), 7),
        ("zero_operations", test_zero_operations(), 42),
        ("fibonacci", test_fibonacci(), 55),
        ("bubblesort", test_bubblesort(), 363),
    ]
}

#[test]
fn scalar_cpu_matches_known_results() {
    for (name, prog, expected) in common_cases() {
        let (a0, cycles) = run_program(prog, MAX_CYCLES);
        assert_eq!(a0, expected, "scalar `{name}`: expected a0={expected}, got {a0} ({cycles} cycles)");
    }
}

#[test]
fn pipelined_cpu_matches_known_results() {
    // Common programs, plus the pipeline-specific ones (the whole point of the
    // pipelined CPU): back-to-back RAW forwarding, a load-use stall, and JAL.
    let mut cases = common_cases();
    cases.push(("jal", pl::test_jal(), 7));
    cases.push(("data_hazard_forwarding", pl::test_data_hazard_forwarding(), 3));
    cases.push(("load_use_stall", pl::test_load_use_stall(), 43));

    for (name, prog, expected) in cases {
        let (a0, cycles) = pl::run_program(prog, MAX_CYCLES);
        assert_eq!(a0, expected, "pipelined `{name}`: expected a0={expected}, got {a0} ({cycles} cycles)");
    }
}

#[test]
fn scalar_and_pipelined_agree() {
    // Differential: on every shared program the two independent microarchitectures
    // must reach the same architectural result. A divergence points at one of them
    // (a hazard/forwarding bug in the pipeline, or a decode bug in either).
    for (name, prog, _expected) in common_cases() {
        let (scalar_a0, _) = run_program(prog.clone(), MAX_CYCLES);
        let (pipelined_a0, _) = pl::run_program(prog, MAX_CYCLES);
        assert_eq!(
            scalar_a0, pipelined_a0,
            "scalar vs pipelined disagree on `{name}`: {scalar_a0} != {pipelined_a0}"
        );
    }
}
