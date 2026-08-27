//! **m4 — the UART receiver's sampling edges, derived from the cycle-dataflow
//! model and pinned against BOTH implementations**
//! (`design_docs/DERIVATION_TABLE.md` §5).
//!
//! The derivation-audit's first cut flagged the uart rows `(read-retime)`. m4's
//! mechanical half showed that to be a **fold artifact**: a folded tick-bearing
//! loop carries interior `uses` but no `defs`, so the mid-bit sample — which
//! inside the sub-CFG both feeds `byte_val`'s commit and steers a branch — looked
//! commit-free from the parent. Under the model every `rx_serial` read is
//! closing-anchored and samples exactly where today's `Deferred` classification
//! samples it. **No retiming.** This file pins the substantive claim behind that:
//! the model's derived sampling edge for every read of a frame, asserted against
//! the simulator AND the transpiled SV.
//!
//! # The derivation (CLKS_PER_BIT = 8, drive-then-clock; `rx(N)` = the value
//! driven before edge N)
//!
//! Every read is a leading (Deferred) read: it parks at its barrier and samples
//! at the pre-edge of the tick that follows it.
//!
//! * **Start detection.** The wait loop reads at pre-edge N each cycle; the first
//!   `rx(N) = 0` breaks it at edge `s`.
//! * **Half-bit delay** consumes edges `s .. s+3` (no reads). The **mid-start
//!   check** parks at the first full-bit-delay tick and samples `rx(s+4)`.
//! * **Full-bit delay** consumes edges `s+4 .. s+11` (no reads).
//! * **Data bit k** (k = 0..7, LSB first): the sample parks at the first tick of
//!   its own delay block and reads `rx(s+12+8k)`; the block consumes edges
//!   `s+12+8k .. s+19+8k`.
//! * **Frame end.** `rx_dv`/`rx_byte` are `RegOut` writes before the bare tick,
//!   committing at edge `s+76`; the trailing `rx_dv.write(Zero)` executes in that
//!   edge's post-edge settle and commits at `s+77`. So `rx_dv` is high at
//!   **observation s+76 exactly**, and `rx_byte` holds Σ b_k·2^k from s+76 on.
//!
//! # The stimulus discriminates the edge
//!
//! Outside the derived sample edges, the wire carries the **complement** of the
//! nearest sample's bit (and the start period carries 1 everywhere except the two
//! derived 0s). A read that lands even one edge early or late therefore sees the
//! wrong value and corrupts the assembled byte — so a pass pins the sampling
//! edges themselves, not just the frame's coarse shape.

mod common;
use common::{verilator_available, verilator_command};
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/uart_rx_dut.rs");

const S: usize = 3; // start-bit detection edge (two idle cycles first)
const BYTE: u8 = 0xB2; // LSB-first bits: 0,1,0,0,1,1,0,1
const TOTAL: usize = S + 78;

/// b_k, LSB first.
fn bit(k: usize) -> u64 {
    ((BYTE >> k) & 1) as u64
}

/// The derived sample edge for data bit k.
fn sample_edge(k: usize) -> usize {
    S + 12 + 8 * k
}

/// The stimulus: `rx(n)` for edge n (1-based).
fn rx(n: usize) -> u64 {
    if n < S {
        return 1; // idle
    }
    if n == S {
        return 0; // start detection
    }
    if n <= S + 3 {
        return 1; // half-bit delay: no reads — adversarial high
    }
    if n == S + 4 {
        return 0; // mid-start check
    }
    if n <= S + 11 {
        return 1; // full-bit delay: no reads
    }
    if n <= S + 75 {
        // Data region: the true bit only at its derived edge; the complement of
        // the NEAREST sample's bit everywhere else.
        let nearest = (0..8)
            .min_by_key(|&k| (n as i64 - sample_edge(k) as i64).abs())
            .unwrap();
        if n == sample_edge(nearest) { bit(nearest) } else { 1 - bit(nearest) }
    } else {
        1 // idle again; also parks the re-entered wait loop
    }
}

/// The derived observation traces.
fn derived_dv(n: usize) -> u64 {
    u64::from(n == S + 76)
}

#[test]
fn m4_uart_sampling_edges_match_the_derived_denotation() {
    // ── Simulator ────────────────────────────────────────────────────────────
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (rx_drv, rx_in) = wire::<Logic, MainClk>(Logic::One);
    let (dv_out, dv_obs) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
    let (byte_out, byte_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh_dv = dv_out.dirty_handle();
    let dh_b = byte_out.dirty_handle();
    let reads = vec![rx_in.wire_id()];
    exec.spawn_wired(
        uart_rx_dut(clk.clone(), rx_in, dv_out, byte_out),
        vec![dh_dv, dh_b],
        reads,
    );

    let mut sim_dv = Vec::new();
    let mut sim_byte = Vec::new();
    for n in 1..=TOTAL {
        rx_drv.write(if rx(n) == 1 { Logic::One } else { Logic::Zero });
        exec.tick_clock(&mut clk);
        sim_dv.push(u64::from(dv_obs.read() == Logic::One));
        sim_byte.push(byte_obs.read().as_u128() as u64);
    }

    let expected_dv: Vec<u64> = (1..=TOTAL).map(derived_dv).collect();
    assert_eq!(
        sim_dv, expected_dv,
        "SIMULATOR: rx_dv is not one-hot at the derived frame-end edge s+76 = {}",
        S + 76
    );
    assert_eq!(
        sim_byte[S + 76 - 1],
        BYTE as u64,
        "SIMULATOR: the assembled byte is not {BYTE:#010b} — some read sampled off \
         its derived edge (each wrong edge sees the complement bit)"
    );

    // ── Transpiled SystemVerilog ─────────────────────────────────────────────
    if !verilator_available() {
        return;
    }
    let sv = copper_codegen::transpile_source(
        include_str!("fixtures/uart_rx_dut.rs"),
        Some("uart_rx_dut"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("transpile");

    let work = std::env::temp_dir().join(format!("copper_m4_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let sv_path = work.join("uart_rx_dut.sv");
    std::fs::write(&sv_path, &sv).unwrap();

    let mut tb = String::from(
        "#include \"Vuart_rx_dut.h\"\n#include \"verilated.h\"\n#include <iostream>\n\
         int main(int c, char** v) { Verilated::commandArgs(c, v);\n\
         Vuart_rx_dut* t = new Vuart_rx_dut(); t->clk = 0; t->eval();\n",
    );
    for n in 1..=TOTAL {
        tb.push_str(&format!(
            "t->rx_serial = {}; t->clk=0; t->eval(); t->clk=1; t->eval(); \
             std::cout << (int)t->rx_dv << \" \" << (int)t->rx_byte << std::endl;\n",
            rx(n)
        ));
    }
    tb.push_str("return 0; }\n");
    let tb_path = work.join("tb.cpp");
    std::fs::write(&tb_path, tb).unwrap();

    let out = verilator_command()
        .current_dir(&work)
        .args([
            "--cc", "--exe", "--build", "--top-module", "uart_rx_dut",
            "-Wno-DECLFILENAME", "-Wno-WIDTHEXPAND", "-CFLAGS", "-std=c++14",
        ])
        .arg(std::fs::canonicalize(&sv_path).unwrap())
        .arg(&tb_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "verilator build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(work.join("obj_dir/Vuart_rx_dut")).output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&work);

    let rows: Vec<(u64, u64)> = stdout
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect();
    let hw_dv: Vec<u64> = rows.iter().map(|r| r.0).collect();
    assert_eq!(
        hw_dv, expected_dv,
        "TRANSPILED SV: rx_dv is not one-hot at the derived frame-end edge s+76 = {}",
        S + 76
    );
    assert_eq!(
        rows[S + 76 - 1].1,
        BYTE as u64,
        "TRANSPILED SV: the assembled byte is not {BYTE:#010b}"
    );
}
