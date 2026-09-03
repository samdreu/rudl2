//! M7 — the fixed-cycle SIMULATION THROUGHPUT benchmark harness: Copper's async
//! simulator vs Verilator AND Icarus Verilog running the SystemVerilog
//! transpiled from the SAME module, over the SAME deterministic stimulus, for a
//! fixed number of cycles. Verilator is the compiled-simulator ceiling; Icarus
//! is the interpreted event-driven baseline the literature compares against.
//!
//! The numbers the example run times could never give (they are dominated by
//! Verilator compilation) are measured here by timing ONLY the cycle loop:
//! excluded on both sides are the Verilator build, model construction, and the
//! untimed boot/reset preamble; included on both sides are stimulus generation
//! and the checksum fold, which are the same trivial integer ops in Rust and C++.
//!
//! **The benchmark is self-checking.** Every run folds the post-edge outputs of
//! every cycle into an FNV-1a-style checksum, on both sides, and the harness
//! asserts they are EQUAL — a throughput comparison between two simulations that
//! computed different things would measure nothing. This makes each benchmark run
//! a long-horizon differential check as a side effect (the corpus sweep runs far
//! fewer cycles per module).
//!
//! Under a bare `cargo test` this runs in SMOKE mode — few cycles, one measured
//! run — so the harness cannot rot and the checksum cross-check is exercised on
//! every regression. The G-C wiring guard in `tools/regression.sh` confirms the
//! binary ran. Real measurements come from `tools/stats/simperf.py`, which sets:
//!
//! ```text
//! COPPER_BENCH_CYCLES=100000 COPPER_BENCH_RUNS=5 COPPER_BENCH_CSV=paper/stats/simperf.csv \
//!     cargo test --release --test sim_throughput -- --nocapture
//! ```
//!
//! Writing the CSV from a debug build is REFUSED — a debug-profile simulator
//! number next to Verilator's -O2 output would be a fabricated comparison.
//!
//! Scope of the comparison (reprinted by the stats summary):
//!   * single-threaded on both sides (Verilator's default; the executor has no
//!     threads), default (levelized) scheduler, `COPPER_SCHEDULER` unset;
//!   * Verilator with its default optimization plus `-CFLAGS -O2`, the Rust side
//!     under the release profile, Icarus via `iverilog -g2012` + `vvp`;
//!   * the Icarus number is process wall-clock MINUS a `+cycles=0` baseline run
//!     of the same binary (vvp has no in-process clock), so startup, bytecode
//!     load, and boot cancel out — see `run_iverilog_side`;
//!   * per-cycle output observation on both sides (the equivalence-test
//!     convention: drive inputs, tick, read post-edge outputs) — this is the
//!     harness-in-the-loop number a testbench author experiences, not a
//!     free-running batch number.

#![allow(clippy::needless_range_loop)]

mod common;
use common::{verilator_available, verilator_command};

// ─── Shared checksum + stimulus primitives (mirrored EXACTLY in the C++ tb) ───

pub const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
pub const XS_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

pub fn fold(csum: u64, v: u64) -> u64 {
    (csum ^ v).wrapping_mul(FNV_PRIME)
}

pub fn xorshift(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

/// The per-design pieces of the loop-form testbenches. Unlike the unrolled
/// testbench `generate_testbench` emits for equivalence checks (one C++ block
/// per recorded cycle — its compile time scales with the trace), these are
/// counted loops, so a million-cycle run compiles in the same time as a
/// ten-cycle one and the compiled stimulus cost stays out of the measurement's
/// way. The C++ fragments drive the Verilated model; the SystemVerilog
/// fragments drive the same stimulus for Icarus — all three sides (and the Rust
/// `sim_rep`) must mirror each other exactly, which the checksum enforces.
pub struct Tb {
    /// C++ file-scope declarations (e.g. a program image).
    pub decl: String,
    /// C++ untimed per-rep preamble: reset/boot, using `top`. Not checksummed.
    pub boot: String,
    /// C++ timed loop body: drive inputs from `i`/`rng`, toggle the clock, fold
    /// post-edge outputs into `csum`.
    pub cycle: String,
    /// SV module-scope declarations: DUT port signals + instantiation (+ data).
    pub sv_decl: String,
    /// SV untimed preamble inside `initial`, using `tick;`.
    pub sv_boot: String,
    /// SV counted-loop body: drive inputs from `ci`/`rng`, `tick;`, fold.
    pub sv_cycle: String,
}

struct Design {
    name: &'static str,
    /// Example source file the module is transpiled from — the same file the
    /// registered example and the corpus sweep use, so the SV benchmarked here
    /// is the SV whose equivalence is already established there.
    src: &'static str,
    module: &'static str,
    /// One measured repetition: fresh executor, untimed boot, then a timed loop
    /// of `cycles` ticks. Returns (elapsed nanoseconds, checksum).
    sim_rep: fn(u64) -> (u128, u64),
    tb: fn() -> Tb,
}

// ─── lfsr: 32-bit LFSR — small sequential datapath ───────────────────────────

#[allow(dead_code, unused_imports)]
mod lfsr_ex {
    include!("../examples/sequential/lfsr.rs");

    pub fn sim_rep(cycles: u64) -> (u128, u64) {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let (reset_drv, reset_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (yumi_drv, yumi_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (o_out, o_obs) = wire::<Bits<32>, MainClk>(Bits::from_u32(1));

        let dh = o_out.dirty_handle();
        let reads = vec![reset_in.wire_id(), yumi_in.wire_id()];
        exec.spawn_wired(lfsr(clk.clone(), reset_in, yumi_in, o_out), vec![dh], reads);

        // Untimed: one reset cycle, then advance every cycle.
        reset_drv.write(Logic::One);
        yumi_drv.write(Logic::Zero);
        exec.tick_clock(&mut clk);
        reset_drv.write(Logic::Zero);
        yumi_drv.write(Logic::One);

        let mut csum = crate::FNV_OFFSET;
        let t0 = std::time::Instant::now();
        for _ in 0..cycles {
            exec.tick_clock(&mut clk);
            csum = crate::fold(csum, o_obs.read().as_u32() as u64);
        }
        (t0.elapsed().as_nanos(), csum)
    }

    pub fn tb() -> crate::Tb {
        crate::Tb {
            decl: String::new(),
            boot: "        top->reset_i = 1; top->yumi_i = 0;\n\
                   \x20       top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                   \x20       top->reset_i = 0; top->yumi_i = 1;\n"
                .into(),
            cycle: "            top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                    \x20           csum = fold(csum, (uint64_t)top->o);\n"
                .into(),
            sv_decl: "    logic reset_i, yumi_i;\n\
                      \x20   logic [31:0] o;\n\
                      \x20   lfsr dut(.clk(clk), .reset_i(reset_i), .yumi_i(yumi_i), .o(o));\n"
                .into(),
            sv_boot: "        reset_i = 1; yumi_i = 0; tick;\n\
                      \x20       reset_i = 0; yumi_i = 1;\n"
                .into(),
            sv_cycle: "            tick;\n\
                       \x20           csum = fold(csum, {32'd0, o});\n"
                .into(),
        }
    }
}

// ─── det_110101: sequence-detector FSM — control-dominated ───────────────────

#[allow(dead_code, unused_imports)]
mod det_ex {
    include!("../examples/sequential/pattern_detector.rs");

    pub fn sim_rep(cycles: u64) -> (u128, u64) {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
        let (in_drv, in_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (out_out, out_obs) = wire::<Logic, MainClk>(Logic::Zero);

        let dh = out_out.dirty_handle();
        let reads = vec![rstn_in.wire_id(), in_in.wire_id()];
        exec.spawn_wired(det_110101(clk.clone(), rstn_in, in_in, out_out), vec![dh], reads);

        // Untimed: one reset cycle.
        rstn_drv.write(Logic::Zero);
        in_drv.write(Logic::Zero);
        exec.tick_clock(&mut clk);
        rstn_drv.write(Logic::One);

        let mut rng = crate::XS_SEED;
        let mut csum = crate::FNV_OFFSET;
        let t0 = std::time::Instant::now();
        for _ in 0..cycles {
            let bit = crate::xorshift(&mut rng) & 1;
            in_drv.write(if bit == 1 { Logic::One } else { Logic::Zero });
            exec.tick_clock(&mut clk);
            csum = crate::fold(csum, (out_obs.read() == Logic::One) as u64);
        }
        (t0.elapsed().as_nanos(), csum)
    }

    pub fn tb() -> crate::Tb {
        crate::Tb {
            decl: String::new(),
            boot: "        top->rstn = 0; top->in_i = 0;\n\
                   \x20       top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                   \x20       top->rstn = 1;\n"
                .into(),
            cycle: "            top->in_i = (int)(xs(rng) & 1);\n\
                    \x20           top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                    \x20           csum = fold(csum, (uint64_t)top->out_o);\n"
                .into(),
            sv_decl: "    logic rstn, in_i, out_o;\n\
                      \x20   det_110101 dut(.clk(clk), .rstn(rstn), .in_i(in_i), .out_o(out_o));\n"
                .into(),
            sv_boot: "        rstn = 0; in_i = 0; tick;\n\
                      \x20       rstn = 1;\n"
                .into(),
            sv_cycle: "            rng = xs_next(rng);\n\
                       \x20           in_i = rng[0];\n\
                       \x20           tick;\n\
                       \x20           csum = fold(csum, {63'd0, out_o});\n"
                .into(),
        }
    }
}

// ─── dual_port_ram: Memory-backed design ─────────────────────────────────────

#[allow(dead_code, unused_imports)]
mod ram_ex {
    include!("../examples/memory/dual_port_ram.rs");

    pub fn sim_rep(cycles: u64) -> (u128, u64) {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let (ena_drv, ena_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (enb_drv, enb_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (wea_drv, wea_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (addra_drv, addra_in) = wire::<Bits<8>, MainClk>(Bits::zero());
        let (addrb_drv, addrb_in) = wire::<Bits<8>, MainClk>(Bits::zero());
        let (dia_drv, dia_in) = wire::<Bits<16>, MainClk>(Bits::zero());
        let (dob_out, dob_obs) = wire::<Bits<16>, MainClk>(Bits::zero());

        let dh: DirtyHandle = dob_out.dirty_handle();
        let reads = vec![
            ena_in.wire_id(), enb_in.wire_id(), wea_in.wire_id(),
            addra_in.wire_id(), addrb_in.wire_id(), dia_in.wire_id(),
        ];
        exec.spawn_wired(
            dual_port_ram(clk.clone(), ena_in, enb_in, wea_in, addra_in, addrb_in, dia_in, dob_out),
            vec![dh],
            reads,
        );

        // Untimed warm-up: write every address once, reads disabled — so the
        // timed loop never reads a never-written cell (whose value would be
        // X in the simulator and unspecified in 2-state Verilator).
        ena_drv.write(Logic::One);
        wea_drv.write(Logic::One);
        for j in 0..256u64 {
            addra_drv.write(Bits::<8>::from_u8(j as u8));
            dia_drv.write(Bits::<16>::from_u16((j.wrapping_mul(0x9E37) & 0xFFFF) as u16));
            exec.tick_clock(&mut clk);
        }
        enb_drv.write(Logic::One);

        let mut csum = crate::FNV_OFFSET;
        let t0 = std::time::Instant::now();
        for i in 0..cycles {
            addra_drv.write(Bits::<8>::from_u8((i & 0xFF) as u8));
            dia_drv.write(Bits::<16>::from_u16((i.wrapping_mul(0x9E37) & 0xFFFF) as u16));
            addrb_drv.write(Bits::<8>::from_u8((i.wrapping_mul(7).wrapping_add(3) & 0xFF) as u8));
            exec.tick_clock(&mut clk);
            csum = crate::fold(csum, dob_obs.read().as_u16() as u64);
        }
        (t0.elapsed().as_nanos(), csum)
    }

    pub fn tb() -> crate::Tb {
        crate::Tb {
            decl: String::new(),
            boot: "        top->enable_a = 1; top->write_a = 1; top->enable_b = 0;\n\
                   \x20       top->addr_b = 0; top->data_in_a = 0;\n\
                   \x20       for (int j = 0; j < 256; j++) {\n\
                   \x20           top->addr_a = j;\n\
                   \x20           top->data_in_a = (int)(((uint64_t)j * 0x9E37ULL) & 0xFFFFULL);\n\
                   \x20           top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                   \x20       }\n\
                   \x20       top->enable_b = 1;\n"
                .into(),
            cycle: "            top->addr_a = (int)(i & 0xFF);\n\
                    \x20           top->data_in_a = (int)((i * 0x9E37ULL) & 0xFFFFULL);\n\
                    \x20           top->addr_b = (int)((i * 7 + 3) & 0xFF);\n\
                    \x20           top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                    \x20           csum = fold(csum, (uint64_t)top->data_out_b);\n"
                .into(),
            sv_decl: "    logic enable_a, enable_b, write_a;\n\
                      \x20   logic [7:0] addr_a, addr_b;\n\
                      \x20   logic [15:0] data_in_a, data_out_b;\n\
                      \x20   dual_port_ram dut(.clk(clk), .enable_a(enable_a), .enable_b(enable_b),\n\
                      \x20       .write_a(write_a), .addr_a(addr_a), .addr_b(addr_b),\n\
                      \x20       .data_in_a(data_in_a), .data_out_b(data_out_b));\n"
                .into(),
            // Same warm-up as the other sides: write every address once so the
            // timed loop never reads a never-written cell — in 4-state Icarus
            // that would fold an X, where 2-state Verilator happened to fold 0.
            sv_boot: "        enable_a = 1; write_a = 1; enable_b = 0; addr_b = 0; data_in_a = 0;\n\
                      \x20       for (bi = 0; bi < 256; bi = bi + 1) begin\n\
                      \x20           addr_a = bi[7:0];\n\
                      \x20           data_in_a = 16'((bi * 64'h9E37) & 64'hFFFF);\n\
                      \x20           tick;\n\
                      \x20       end\n\
                      \x20       enable_b = 1;\n"
                .into(),
            sv_cycle: "            addr_a = 8'(ci & 64'hFF);\n\
                       \x20           data_in_a = 16'((ci * 64'h9E37) & 64'hFFFF);\n\
                       \x20           addr_b = 8'((ci * 64'd7 + 64'd3) & 64'hFF);\n\
                       \x20           tick;\n\
                       \x20           csum = fold(csum, {48'd0, data_out_b});\n"
                .into(),
        }
    }
}

// ─── rv32i_cpu_transpilable: CPU-scale design ────────────────────────────────

#[allow(dead_code, unused_imports)]
mod cpu_ex {
    include!("../examples/cpu/rv32i_cpu_transpilable.rs");

    /// A NON-HALTING workload: a counter loop with a store, a load (which
    /// stalls), ALU ops over the loaded value, and branches every iteration —
    /// so fetch, forwarding, the load-use stall, memory traffic, and the branch
    /// flush are all exercised continuously.
    ///
    /// The folded outputs are `program_counter` and `a0_out` — and `a0_out` is
    /// the RESULT register, latched only at ecall, so in a non-halting program
    /// it folds a constant and the pc stream carries all the sensitivity. The
    /// `beq` therefore branches on the low bits of the LOADED value: the pc
    /// sequence (period 4 iterations, unequal iteration lengths) is
    /// data-dependent, so a datapath error changes the checksum rather than
    /// hiding behind a fixed control pattern.
    pub fn bench_program() -> Vec<u32> {
        vec![
            addi(10, 0, 0),                    //  0:        x10 = 0
            addi(12, 0, 0),                    //  4:        x12 = 0
            addi(10, 10, 1),                   //  8: loop:  x10 += 1
            sw(0, 10, 256),                    // 12:        mem[256] = x10
            lw(11, 0, 256),                    // 16:        x11 = mem[256]
            i_type(13, 11, 3, 0x7, 0x13),      // 20:        andi x13, x11, 3
            beq(13, 0, 8),                     // 24:        x13 == 0 → skip (32)
            add(12, 12, 11),                   // 28:        x12 += x11
            bne(10, 0, -24),                   // 32:        x10 != 0 → loop (8)
        ]
    }

    pub fn sim_rep(cycles: u64) -> (u128, u64) {
        let program = bench_program();
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (ben_drv, ben_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (baddr_drv, baddr_in) = wire::<Bits<32>, MainClk>(Bits::zero());
        let (bdata_drv, bdata_in) = wire::<Bits<32>, MainClk>(Bits::zero());
        let (pc_out, pc_in) = wire::<Bits<32>, MainClk>(Bits::zero());
        let (halt_out, _halt_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (a0_out, a0_in) = wire::<Bits<32>, MainClk>(Bits::zero());

        let reads = vec![
            rstn_in.wire_id(), ben_in.wire_id(), baddr_in.wire_id(), bdata_in.wire_id(),
        ];
        exec.spawn_untracked(
            rv32i_cpu_transpilable(
                clk.clone(), rstn_in, ben_in, baddr_in, bdata_in, pc_out, halt_out, a0_out,
            ),
            reads,
        );

        // Untimed boot — the same sequence `run_program` uses: one word per
        // cycle with rstn held low, one flush cycle, then release reset.
        for (i, word) in program.iter().enumerate() {
            ben_drv.write(Logic::One);
            baddr_drv.write(Bits::<32>::from_usize(i * 4));
            bdata_drv.write(Bits::<32>::from_u32(*word));
            exec.tick_clock(&mut clk);
        }
        ben_drv.write(Logic::Zero);
        exec.tick_clock(&mut clk);
        rstn_drv.write(Logic::One);

        // 16 post-reset warm cycles before folding starts, on EVERY side. The
        // emitted SV leaves registers uninitialized (reset or write-before-read
        // covers correctness), which 2-state Verilator zero-fills — matching the
        // simulator's zero inits by accident — but 4-state Icarus keeps as X.
        // The pipeline-fill window is where an X could slip through a path the
        // reset does not drive; by cycle 16 every architectural register the
        // workload reads has been written, so all three simulators fold only
        // defined values.
        for _ in 0..16 {
            exec.tick_clock(&mut clk);
        }

        let mut csum = crate::FNV_OFFSET;
        let t0 = std::time::Instant::now();
        for _ in 0..cycles {
            exec.tick_clock(&mut clk);
            csum = crate::fold(csum, pc_in.read().as_u64());
            csum = crate::fold(csum, a0_in.read().as_u64());
        }
        (t0.elapsed().as_nanos(), csum)
    }

    pub fn tb() -> crate::Tb {
        let prog = bench_program();
        let words: Vec<String> = prog.iter().map(|w| format!("0x{w:08x}u")).collect();
        crate::Tb {
            decl: format!(
                "static const uint32_t PROG[] = {{ {} }};\nstatic const int PROG_LEN = {};",
                words.join(", "),
                prog.len()
            ),
            boot: "        top->rstn = 0; top->boot_en = 0; top->boot_addr = 0; top->boot_data = 0;\n\
                   \x20       for (int j = 0; j < PROG_LEN; j++) {\n\
                   \x20           top->boot_en = 1;\n\
                   \x20           top->boot_addr = (uint32_t)(j * 4);\n\
                   \x20           top->boot_data = PROG[j];\n\
                   \x20           top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                   \x20       }\n\
                   \x20       top->boot_en = 0;\n\
                   \x20       top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                   \x20       top->rstn = 1;\n\
                   \x20       for (int j = 0; j < 16; j++) { // X-flush warm-up, see sim_rep\n\
                   \x20           top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                   \x20       }\n"
                .into(),
            cycle: "            top->clk = 0; top->eval(); top->clk = 1; top->eval();\n\
                    \x20           csum = fold(csum, (uint64_t)top->program_counter);\n\
                    \x20           csum = fold(csum, (uint64_t)top->a0_out);\n"
                .into(),
            sv_decl: format!(
                "    logic rstn, boot_en, halted;\n\
                 \x20   logic [31:0] boot_addr, boot_data, program_counter, a0_out;\n\
                 \x20   logic [31:0] PROG [0:{}];\n\
                 \x20   rv32i_cpu_transpilable dut(.clk(clk), .rstn(rstn), .boot_en(boot_en),\n\
                 \x20       .boot_addr(boot_addr), .boot_data(boot_data),\n\
                 \x20       .program_counter(program_counter), .halted(halted), .a0_out(a0_out));\n",
                prog.len() - 1
            ),
            sv_boot: {
                let mut b = String::from(
                    "        rstn = 0; boot_en = 0; boot_addr = 0; boot_data = 0;\n",
                );
                for (j, w) in prog.iter().enumerate() {
                    b.push_str(&format!("        PROG[{j}] = 32'h{w:08x};\n"));
                }
                b.push_str(&format!(
                    "        for (bi = 0; bi < {}; bi = bi + 1) begin\n\
                     \x20           boot_en = 1;\n\
                     \x20           boot_addr = 32'(bi * 64'd4);\n\
                     \x20           boot_data = PROG[bi];\n\
                     \x20           tick;\n\
                     \x20       end\n\
                     \x20       boot_en = 0; tick;\n\
                     \x20       rstn = 1;\n\
                     \x20       for (bi = 0; bi < 16; bi = bi + 1) tick; // X-flush warm-up\n",
                    prog.len()
                ));
                b
            },
            sv_cycle: "            tick;\n\
                       \x20           csum = fold(csum, {32'd0, program_counter});\n\
                       \x20           csum = fold(csum, {32'd0, a0_out});\n"
                .into(),
        }
    }
}

// ─── The C++ testbench template ──────────────────────────────────────────────

/// `+cycles=`/`+runs=` plusargs; per rep: fresh model, untimed boot, timed loop,
/// one `RUN ns=<dec> csum=<hex>` line. `fold`/`xs` are byte-for-byte the Rust
/// `fold`/`xorshift` above — the checksum comparison depends on it.
fn render_tb(module: &str, tb: &Tb) -> String {
    let template = r#"#include "V@MODULE@.h"
#include "verilated.h"
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <string>

static inline uint64_t fold(uint64_t c, uint64_t v) { return (c ^ v) * 0x100000001B3ULL; }
static inline uint64_t xs(uint64_t& x) { x ^= x << 13; x ^= x >> 7; x ^= x << 17; return x; }

@DECL@

int main(int argc, char** argv) {
    Verilated::commandArgs(argc, argv);
    // COPY each plusarg before fetching the next: commandArgsPlusMatch returns a
    // pointer into one reused internal string, so after the second call the
    // first pointer reads "+runs=K". Skipping a fixed 8 bytes from that landed
    // on the terminator for a two-digit K (cycles parsed as 0, the loop never
    // ran, and the checksum stayed at the FNV offset) and, for a one-digit K, on
    // the previous string's leftover digits — which is why every run with fewer
    // than nine repetitions passed by accident. Found 2026-09-03 on the first
    // ten-repetition run. Parse after '=' rather than at a fixed offset.
    std::string cyS = Verilated::commandArgsPlusMatch("cycles=");
    std::string rnS = Verilated::commandArgsPlusMatch("runs=");
    if (cyS.empty() || rnS.empty()) { fprintf(stderr, "need +cycles=N and +runs=K\n"); return 2; }
    uint64_t cycles = strtoull(cyS.c_str() + cyS.find('=') + 1, 0, 10);
    long runs = strtol(rnS.c_str() + rnS.find('=') + 1, 0, 10);
    for (long rep = 0; rep < runs; rep++) {
        V@MODULE@* top = new V@MODULE@();
        uint64_t csum = 0xCBF29CE484222325ULL;
        uint64_t rng = 0x9E3779B97F4A7C15ULL;
        (void)rng;
@BOOT@
        auto t0 = std::chrono::steady_clock::now();
        for (uint64_t i = 0; i < cycles; i++) {
            (void)i;
@CYCLE@
        }
        auto t1 = std::chrono::steady_clock::now();
        unsigned long long ns = (unsigned long long)
            std::chrono::duration_cast<std::chrono::nanoseconds>(t1 - t0).count();
        // Report the cycle count the loop actually used, so the harness can
        // refuse a run that measured something other than what it asked for.
        printf("RUN cycles=%llu ns=%llu csum=%016llx\n", (unsigned long long)cycles, ns, (unsigned long long)csum);
        top->final();
        delete top;
    }
    return 0;
}
"#;
    template
        .replace("@MODULE@", module)
        .replace("@DECL@", &tb.decl)
        .replace("@BOOT@", &tb.boot)
        .replace("@CYCLE@", &tb.cycle)
}

/// The SystemVerilog testbench for Icarus — the same shape as the C++ one:
/// `+cycles=` plusarg, untimed boot, counted checksummed loop, one final
/// `RUN csum=<hex>` line. `fold`/`xs_next` are bit-for-bit the Rust
/// `fold`/`xorshift` (verified: identical checksums for identical streams).
/// No per-rep loop here: repetitions are separate `vvp` processes, timed from
/// outside and corrected by a `+cycles=0` baseline (see `run_iverilog_side`).
fn render_sv_tb(tb: &Tb) -> String {
    let template = r#"module bench_tb;
    logic clk = 0;
@DECL@
    longint unsigned csum, rng, ncycles, ci, bi;
    integer cycles_i;

    function automatic longint unsigned fold(longint unsigned c, longint unsigned v);
        return (c ^ v) * 64'h100000001B3;
    endfunction
    function automatic longint unsigned xs_next(longint unsigned x);
        x = x ^ (x << 13); x = x ^ (x >> 7); x = x ^ (x << 17); return x;
    endfunction
    task tick; begin clk = 0; #1; clk = 1; #1; end endtask

    initial begin
        if (!$value$plusargs("cycles=%d", cycles_i)) begin
            $display("need +cycles=N");
            $finish;
        end
        ncycles = cycles_i;
        csum = 64'hCBF29CE484222325;
        rng  = 64'h9E3779B97F4A7C15;
@BOOT@
        for (ci = 0; ci < ncycles; ci = ci + 1) begin
@CYCLE@
        end
        $display("RUN csum=%h", csum);
        $finish;
    end
endmodule
"#;
    template
        .replace("@DECL@", &tb.sv_decl)
        .replace("@BOOT@", &tb.sv_boot)
        .replace("@CYCLE@", &tb.sv_cycle)
}

// ─── Verilator side ──────────────────────────────────────────────────────────

/// Transpile the module the benchmark's Verilog sides will run — once per
/// design, shared by Verilator and Icarus so both simulate the same bytes.
fn transpile(d: &Design) -> String {
    let src = std::fs::read_to_string(d.src)
        .unwrap_or_else(|e| panic!("{}: cannot read {}: {e}", d.name, d.src));
    copper_codegen::transpile_source(
        &src,
        Some(d.module),
        &copper_codegen::EmitConfig::default(),
    )
    .unwrap_or_else(|e| panic!("{}: transpile failed: {e}", d.name))
}

/// Build the loop-form C++ testbench (untimed), run it for `reps` repetitions
/// of `cycles` cycles, and parse the per-rep results.
fn run_verilator_side(d: &Design, sv: &str, cycles: u64, reps: u32) -> Vec<(u128, u64)> {
    // Per-invocation work dir, per the repo rule: never key on less than
    // (module, pid) — two concurrent runs must not share a build directory.
    let work = std::path::PathBuf::from(format!(
        "target/verilator/bench_{}_{}",
        d.module,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join(format!("{}.sv", d.module)), sv).unwrap();
    std::fs::write(work.join("tb.cpp"), render_tb(d.module, &(d.tb)())).unwrap();

    let out = verilator_command()
        .current_dir(&work)
        .args([
            "--cc", "--exe", "--build",
            "--top-module", d.module,
            "-Wno-DECLFILENAME",
            "-CFLAGS", "-std=c++14 -O2",
        ])
        .arg(format!("{}.sv", d.module))
        .arg("tb.cpp")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}: Verilator build failed (work dir kept at {}):\n{}",
        d.name,
        work.display(),
        String::from_utf8_lossy(&out.stderr)
    );

    let run = std::process::Command::new(work.join(format!("obj_dir/V{}", d.module)))
        .arg(format!("+cycles={cycles}"))
        .arg(format!("+runs={reps}"))
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}: Verilated benchmark run failed:\n{}",
        d.name,
        String::from_utf8_lossy(&run.stderr)
    );

    let results: Vec<(u128, u64)> = String::from_utf8_lossy(&run.stdout)
        .lines()
        .filter_map(|l| {
            let l = l.strip_prefix("RUN cycles=")?;
            let (ran, rest) = l.split_once(" ns=")?;
            let ran: u64 = ran.parse().ok()?;
            assert_eq!(
                ran, cycles,
                "{}: the Verilated benchmark ran {ran} cycles, not the {cycles} requested — \
                 its plusarg parsing is wrong, and a checksum over the wrong horizon proves nothing",
                d.name
            );
            let (ns, csum) = rest.split_once(" csum=")?;
            Some((ns.parse().ok()?, u64::from_str_radix(csum, 16).ok()?))
        })
        .collect();
    assert_eq!(
        results.len(),
        reps as usize,
        "{}: expected {} RUN lines from the Verilated benchmark, got {}",
        d.name,
        reps,
        results.len()
    );

    let _ = std::fs::remove_dir_all(&work);
    results
}

// ─── Icarus Verilog side ─────────────────────────────────────────────────────

/// Whether `iverilog` is present and runnable — the same three-way split as
/// `verilator_status`: absent is skippable, present-but-broken must fail.
fn iverilog_available() -> bool {
    match std::process::Command::new("iverilog").arg("-V").output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => panic!(
            "iverilog is installed but `iverilog -V` failed — a broken environment, \
             not a missing tool, and must not be skipped:\n{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(_) => false,
    }
}

/// One `vvp` invocation: returns (process wall-clock ns, checksum if printed).
fn run_vvp(vvp: &std::path::Path, work: &std::path::Path, cycles: u64, name: &str) -> (u128, Option<u64>) {
    let t0 = std::time::Instant::now();
    let out = std::process::Command::new("vvp")
        .arg(vvp)
        .arg(format!("+cycles={cycles}"))
        .current_dir(work)
        .output()
        .unwrap();
    let ns = t0.elapsed().as_nanos();
    assert!(
        out.status.success(),
        "{name}: vvp run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let csum = stdout.lines().find_map(|l| l.strip_prefix("RUN csum=")).map(|h| {
        u64::from_str_radix(h.trim(), 16).unwrap_or_else(|_| {
            panic!(
                "{name}: Icarus printed a non-numeric checksum ({h:?}) — an `x` here \
                 means 4-state X reached a folded output that 2-state simulation \
                 zero-filled; extend the design's warm-up rather than the parser"
            )
        })
    });
    (ns, csum)
}

/// Icarus: `iverilog -g2012` compile (untimed), then per rep one `vvp` process.
/// vvp has no in-process clock a testbench can read, so each rep's time is
/// process wall-clock MINUS the median of three `+cycles=0` baseline runs of
/// the same binary — startup, bytecode load, and the untimed boot cancel out.
/// Noise can exceed the loop cost at smoke-mode cycle counts (the checksum
/// check still stands); at measurement cycle counts the loop dominates.
fn run_iverilog_side(d: &Design, sv: &str, cycles: u64, reps: u32) -> Vec<(u128, u64)> {
    let work = std::path::PathBuf::from(format!(
        "target/verilator/bench_{}_iv_{}",
        d.module,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join(format!("{}.sv", d.module)), sv).unwrap();
    std::fs::write(work.join("tb.sv"), render_sv_tb(&(d.tb)())).unwrap();

    let out = std::process::Command::new("iverilog")
        .current_dir(&work)
        .args(["-g2012", "-s", "bench_tb", "-o", "bench.vvp", "tb.sv"])
        .arg(format!("{}.sv", d.module))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}: iverilog compile failed (work dir kept at {}):\n{}",
        d.name,
        work.display(),
        String::from_utf8_lossy(&out.stderr)
    );

    let vvp = std::path::PathBuf::from("bench.vvp");
    let mut baselines: Vec<u128> = (0..3).map(|_| run_vvp(&vvp, &work, 0, d.name).0).collect();
    baselines.sort_unstable();
    let baseline = baselines[1];

    let mut results = Vec::new();
    for rep in 0..=reps {
        let (ns, csum) = run_vvp(&vvp, &work, cycles, d.name);
        let csum = csum.unwrap_or_else(|| {
            panic!("{}: vvp printed no `RUN csum=` line", d.name)
        });
        if rep > 0 {
            results.push((ns.saturating_sub(baseline).max(1), csum));
        }
    }

    let _ = std::fs::remove_dir_all(&work);
    results
}

// ─── Harness ─────────────────────────────────────────────────────────────────

fn median_ns(mut runs: Vec<u128>) -> u128 {
    runs.sort_unstable();
    runs[runs.len() / 2]
}

fn cps(cycles: u64, ns: u128) -> f64 {
    cycles as f64 * 1e9 / ns as f64
}

#[test]
fn simulation_throughput() {
    let env_u64 = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<u64>().ok());
    // Smoke defaults: enough cycles to be a real long-horizon differential
    // check, small enough not to weigh on `cargo test`.
    let cycles = env_u64("COPPER_BENCH_CYCLES").unwrap_or(2_000);
    let runs = env_u64("COPPER_BENCH_RUNS").unwrap_or(1) as u32;
    let csv_path = std::env::var("COPPER_BENCH_CSV").ok();
    assert!(runs >= 1, "COPPER_BENCH_RUNS must be >= 1");

    if csv_path.is_some() && cfg!(debug_assertions) {
        panic!(
            "refusing to write the benchmark CSV from a DEBUG build — a debug-profile \
             simulator number next to Verilator's -O2 output is not a comparison. \
             Run via tools/stats/simperf.py (which passes --release)."
        );
    }

    let designs = [
        Design {
            name: "lfsr",
            src: "examples/sequential/lfsr.rs",
            module: "lfsr",
            sim_rep: lfsr_ex::sim_rep,
            tb: lfsr_ex::tb,
        },
        Design {
            name: "det_110101",
            src: "examples/sequential/pattern_detector.rs",
            module: "det_110101",
            sim_rep: det_ex::sim_rep,
            tb: det_ex::tb,
        },
        Design {
            name: "dual_port_ram",
            src: "examples/memory/dual_port_ram.rs",
            module: "dual_port_ram",
            sim_rep: ram_ex::sim_rep,
            tb: ram_ex::tb,
        },
        Design {
            name: "rv32i_cpu_transpilable",
            src: "examples/cpu/rv32i_cpu_transpilable.rs",
            module: "rv32i_cpu_transpilable",
            sim_rep: cpu_ex::sim_rep,
            tb: cpu_ex::tb,
        },
    ];

    let have_verilator = verilator_available();
    let have_iverilog = iverilog_available();
    let mut csv_rows: Vec<String> = Vec::new();

    for d in &designs {
        println!("── {} — {cycles} cycles × {runs} run(s) + warmup ──", d.name);
        let sv = transpile(d);

        // Simulator side: one warmup rep (discarded), then `runs` measured reps.
        let mut sim: Vec<(u128, u64)> = Vec::new();
        for rep in 0..=runs {
            let r = (d.sim_rep)(cycles);
            if rep > 0 {
                println!("  sim       {:>12.0} cycles/s  (csum {:016x})", cps(cycles, r.0), r.1);
                sim.push(r);
            }
        }
        let sim_csum = sim[0].1;
        assert!(
            sim.iter().all(|r| r.1 == sim_csum),
            "{}: simulator checksums differ across repetitions — the simulation is \
             not deterministic",
            d.name
        );

        // A Verilog side: drop the warmup rep, require internally consistent
        // checksums, and require agreement with the simulator.
        let check = |mut v: Vec<(u128, u64)>, side: &str| -> Vec<(u128, u64)> {
            v.remove(0); // warmup
            for r in &v {
                println!("  {side:<9} {:>12.0} cycles/s  (csum {:016x})", cps(cycles, r.0), r.1);
            }
            let csum = v[0].1;
            assert!(
                v.iter().all(|r| r.1 == csum),
                "{}: {side} checksums differ across repetitions",
                d.name
            );
            assert_eq!(
                csum, sim_csum,
                "{}: SIMULATOR AND {} DISAGREE over {} cycles — this is a sim≢SV \
                 divergence, not a benchmark problem; do not tune around it",
                d.name,
                side.to_uppercase(),
                cycles
            );
            v
        };

        let ver = if have_verilator {
            Some(check(run_verilator_side(d, &sv, cycles, runs + 1), "verilator"))
        } else {
            println!("  verilator skipped (not installed)");
            None
        };
        let iv = if have_iverilog {
            Some(check(run_iverilog_side(d, &sv, cycles, runs + 1), "iverilog"))
        } else {
            println!("  iverilog skipped (not installed)");
            None
        };

        let sim_med = median_ns(sim.iter().map(|r| r.0).collect());
        // (median ns as string, cycles/s as string) for an optional side.
        let side_cols = |v: &Option<Vec<(u128, u64)>>| match v {
            Some(v) => {
                let m = median_ns(v.iter().map(|r| r.0).collect());
                (m, m.to_string(), format!("{:.0}", cps(cycles, m)))
            }
            None => (0, String::new(), String::new()),
        };
        let (ver_m, ver_med, ver_cps) = side_cols(&ver);
        let (iv_m, iv_med, iv_cps) = side_cols(&iv);
        // Both ratios read "left is N× faster than right".
        let ver_over_sim =
            if ver.is_some() { format!("{:.1}", sim_med as f64 / ver_m as f64) } else { String::new() };
        let sim_over_iv =
            if iv.is_some() { format!("{:.1}", iv_m as f64 / sim_med as f64) } else { String::new() };

        println!(
            "  ── median: sim {:.0} cycles/s{}{}",
            cps(cycles, sim_med),
            if ver.is_some() {
                format!(", verilator {ver_cps} (verilator/sim {ver_over_sim}x)")
            } else {
                String::new()
            },
            if iv.is_some() {
                format!(", iverilog {iv_cps} (sim/iverilog {sim_over_iv}x)")
            } else {
                String::new()
            },
        );

        csv_rows.push(format!(
            "{},{cycles},{runs},{sim_med},{ver_med},{:.0},{ver_cps},{ver_over_sim},\
             {sim_csum:016x},{iv_med},{iv_cps},{sim_over_iv}",
            d.name,
            cps(cycles, sim_med),
        ));
    }

    if let Some(path) = csv_path {
        assert!(
            have_verilator && have_iverilog,
            "COPPER_BENCH_CSV is set but a Verilog simulator is missing (verilator: {}, \
             iverilog: {}) — the metric IS the comparison, so there is nothing honest \
             to write; install the missing one",
            have_verilator,
            have_iverilog
        );
        let header = "design,cycles,runs,sim_ns_median,verilator_ns_median,\
                      sim_cycles_per_sec,verilator_cycles_per_sec,verilator_over_sim,\
                      checksum,iverilog_ns_median,iverilog_cycles_per_sec,sim_over_iverilog";
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, format!("{header}\n{}\n", csv_rows.join("\n"))).unwrap();
        println!("-> {path}");
    }
}
