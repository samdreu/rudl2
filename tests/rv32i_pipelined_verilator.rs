//! **The pipelined CPU's sweep lane**: simulator vs transpiled SystemVerilog,
//! cycle-for-cycle, under Verilator — the anchoring the corpus sweep cannot do
//! because the core RECEIVES its memory as a parameter (build.rs `SKIP`
//! records exactly that gap; this closes it).
//!
//! Shape: `tests/received_memory_abi.rs` scaled up. The transpiled core is
//! instantiated under a hand-written OWNER that provides what any RAM wrapper
//! provides — the array, continuous reads, and a guarded non-blocking commit —
//! plus the collision policy, which for THIS core is **WriteFirst**
//! (`build_memory` calls `.write_first()`), so each read port forwards a
//! same-edge write to the same address:
//!
//! ```systemverilog
//! assign rdN_data = (wr0_en && rdN_addr == wr0_addr) ? wr0_data : mem[rdN_addr];
//! ```
//!
//! The policy lives in the OWNER on purpose: the `Memory<…>` TYPE carries no
//! write mode (`.write_first()` is a runtime builder), and the received-memory
//! ABI leaves every array-side decision to whoever owns the array.
//!
//! All 13 architectural programs run on both sides; the trace compared is
//! `(program_counter, halted, a0_out)` at every post-edge observation, through
//! the halt and a few parked cycles past it. One Verilator build serves every
//! program: the image loads via `$readmemh` from a `+prog=<file>` plusarg.

#![allow(dead_code)] // the included example carries a full harness; this uses a subset

mod common;
use common::{verilator_available, verilator_command};

// The example's `fn main` is `#[cfg(not(test))]`, so the include brings the
// core, the assemblers, the 13 programs, and `build_memory` — the same pattern
// `rv32i_integration.rs` uses.
mod pl {
    include!("../examples/cpu/rv32i_cpu_pipelined.rs");
    // (`registered_wire`, `wire`, the core, the assemblers, and the 13
    // `test_*` programs all arrive with the include.)

    /// One architectural program per entry: `(name, image, expected a0)`.
    pub fn programs() -> Vec<(&'static str, Vec<u32>, u32)> {
        vec![
            ("addi", test_addi(), 15),
            ("sub", test_sub(), 5),
            ("multiple_adds", test_multiple_adds(), 15),
            ("branch_taken", test_branch_taken(), 42),
            ("branch_not_taken", test_branch_not_taken(), 99),
            ("load_store", test_load_store(), 88),
            ("negative", test_negative_numbers(), 7),
            ("zero", test_zero_operations(), 42),
            ("jal", test_jal(), 7),
            ("forwarding", test_data_hazard_forwarding(), 3),
            ("load_use_stall", test_load_use_stall(), 43),
            ("fibonacci", test_fibonacci(), 55),
            ("bubblesort", test_bubblesort(), 363),
        ]
    }

    /// Run the SIMULATOR and capture `(pc, halted, a0)` at every post-edge
    /// observation, until `extra` cycles past the halt (the parked state must
    /// match too). Panics if the program never halts — same contract as
    /// `run_program`.
    pub fn trace_program(program: Vec<u32>, max_cycles: usize, extra: usize) -> Vec<(u32, u8, u32)> {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();
        let memory = build_memory(&clk, program);

        let (pc_out, pc_in) = registered_wire::<Bits<32>, MainClk>(&clk, Bits::zero());
        let (halt_out, halt_in) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
        let (a0_out, a0_in) = registered_wire::<Bits<32>, MainClk>(&clk, Bits::zero());

        exec.spawn_untracked(
            rv32i_cpu_pipelined(clk.clone(), memory, pc_out, halt_out, a0_out),
            vec![],
        );

        let mut trace = Vec::new();
        let mut parked = 0usize;
        for _ in 1..=max_cycles {
            exec.tick_clock(&mut clk);
            let halted = if halt_in.read() == Logic::One { 1u8 } else { 0u8 };
            trace.push((pc_in.read().as_u32(), halted, a0_in.read().as_u32()));
            if halted == 1 {
                parked += 1;
                if parked > extra {
                    return trace;
                }
            }
        }
        panic!("program did not halt within {max_cycles} cycles");
    }
}

/// The owner: the array (preloaded via `$readmemh`), WRITE-FIRST continuous
/// reads, and the guarded commit. `MEM_WORDS` = 1024 → `MEMORY_ADDR_W` = 10.
const OWNER_SV: &str = r#"
module owner_top(
    input  logic clk,
    output logic [31:0] program_counter,
    output logic halted,
    output logic [31:0] a0_out
);
    logic [31:0] mem [0:1023];
    logic [9:0]  memory_rd0_addr;
    logic [31:0] memory_rd0_data;
    logic [9:0]  memory_rd1_addr;
    logic [31:0] memory_rd1_data;
    logic        memory_wr0_en;
    logic [9:0]  memory_wr0_addr;
    logic [31:0] memory_wr0_data;

    rv32i_cpu_pipelined #(.MEMORY_ADDR_W(10)) core (
        .clk(clk),
        .program_counter(program_counter), .halted(halted), .a0_out(a0_out),
        .memory_rd0_addr(memory_rd0_addr), .memory_rd0_data(memory_rd0_data),
        .memory_rd1_addr(memory_rd1_addr), .memory_rd1_data(memory_rd1_data),
        .memory_wr0_en(memory_wr0_en), .memory_wr0_addr(memory_wr0_addr),
        .memory_wr0_data(memory_wr0_data)
    );

    // WriteFirst: a read of the address being written this edge captures the
    // NEW word — matching `build_memory`'s `.write_first()`.
    assign memory_rd0_data =
        (memory_wr0_en && memory_rd0_addr == memory_wr0_addr) ? memory_wr0_data
                                                              : mem[memory_rd0_addr];
    assign memory_rd1_data =
        (memory_wr0_en && memory_rd1_addr == memory_wr0_addr) ? memory_wr0_data
                                                              : mem[memory_rd1_addr];
    always_ff @(posedge clk) if (memory_wr0_en) mem[memory_wr0_addr] <= memory_wr0_data;

    initial begin
        string prog;
        for (int i = 0; i < 1024; i++) mem[i] = '0;
        if ($value$plusargs("prog=%s", prog)) $readmemh(prog, mem);
    end
endmodule
"#;

#[test]
fn pipelined_cpu_matches_its_verilated_self_on_all_programs() {
    // The simulator side runs (and self-checks a0) regardless of Verilator.
    let extra = 4usize;
    let programs = pl::programs();
    let mut sim_traces = Vec::new();
    for (name, image, expected_a0) in &programs {
        let trace = pl::trace_program(image.clone(), 2000, extra);
        let last = trace.last().unwrap();
        assert_eq!(last.1, 1, "{name}: simulator trace must end halted");
        assert_eq!(
            last.2, *expected_a0,
            "{name}: simulator a0 disagrees with the architectural result"
        );
        sim_traces.push(trace);
    }

    if !verilator_available() {
        return;
    }

    // ── One Verilator build for every program ────────────────────────────────
    let child_sv = copper_codegen::transpile_source(
        &std::fs::read_to_string("examples/cpu/rv32i_cpu_pipelined.rs").unwrap(),
        Some("rv32i_cpu_pipelined"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("the pipelined CPU must transpile");

    let work = std::env::temp_dir().join(format!("copper_rv32i_pl_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("rv32i_cpu_pipelined.sv"), &child_sv).unwrap();
    std::fs::write(work.join("owner_top.sv"), OWNER_SV).unwrap();

    // `+cycles=N` bounds the run; one `pc halted a0` line per cycle.
    let tb = "#include \"Vowner_top.h\"\n#include \"verilated.h\"\n#include <iostream>\n\
        int main(int c, char** v) { Verilated::commandArgs(c, v);\n\
        Vowner_top* t = new Vowner_top();\n\
        const char* cy = Verilated::commandArgsPlusMatch(\"cycles=\");\n\
        int n = atoi(cy + 8);\n\
        t->clk = 0; t->eval();\n\
        for (int i = 0; i < n; i++) {\n\
            t->clk = 0; t->eval(); t->clk = 1; t->eval();\n\
            std::cout << (unsigned)t->program_counter << ' ' << (int)t->halted\n\
                      << ' ' << (unsigned)t->a0_out << std::endl;\n\
        }\n\
        return 0; }\n";
    std::fs::write(work.join("tb.cpp"), tb).unwrap();

    let out = verilator_command()
        .current_dir(&work)
        .args([
            "--cc", "--exe", "--build", "--top-module", "owner_top",
            "-Wno-DECLFILENAME", "-CFLAGS", "-std=c++14",
        ])
        .arg(work.join("owner_top.sv"))
        .arg(work.join("rv32i_cpu_pipelined.sv"))
        .arg(work.join("tb.cpp"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "verilator build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // ── Per program: write the image, run, compare cycle-for-cycle ──────────
    for ((name, image, _), sim) in programs.iter().zip(&sim_traces) {
        let hex: String = image.iter().map(|w| format!("{w:08x}\n")).collect();
        let prog_path = work.join(format!("{name}.hex"));
        std::fs::write(&prog_path, hex).unwrap();

        let run = std::process::Command::new(work.join("obj_dir/Vowner_top"))
            .arg(format!("+prog={}", prog_path.display()))
            .arg(format!("+cycles={}", sim.len()))
            .output()
            .unwrap();
        assert!(run.status.success(), "{name}: Verilated run failed");
        let hw: Vec<(u32, u8, u32)> = String::from_utf8_lossy(&run.stdout)
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?))
            })
            .collect();
        assert_eq!(
            hw.len(),
            sim.len(),
            "{name}: Verilated trace has {} cycles, simulator has {}",
            hw.len(),
            sim.len()
        );
        for (cycle, (h, s)) in hw.iter().zip(sim.iter()).enumerate() {
            assert_eq!(
                h, s,
                "{name}: cycle {cycle} diverges — Verilated (pc, halted, a0) = \
                 {h:?}, simulator = {s:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&work);
}
