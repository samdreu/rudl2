//! **The received-`Memory` port ABI, verified end to end** (cause P;
//! `design_docs/RECEIVED_MEMORY_ABI.md`).
//!
//! A module handed its storage as a `Memory<…>` PARAMETER emits a **bus**
//! instead of an array: per used read port `<m>_rd<i>_addr` (out, width
//! `M_ADDR_W`, a module parameter — the depth is the owner's runtime argument
//! and is not in the type) and `<m>_rd<i>_data` (in); per used write port
//! `<m>_wr<j>_{en,addr,data}` (out). Every timing decision stays on the
//! module's side — the staging `always_comb` and the read-capture registers
//! clock the module's own edge — and the OWNER provides exactly what any RAM
//! wrapper provides:
//!
//! ```systemverilog
//! assign  <m>_rd<i>_data = mem[<m>_rd<i>_addr];                  // continuous read
//! always_ff @(posedge clk) if (<m>_wr<j>_en) mem[<m>_wr<j>_addr] <= <m>_wr<j>_data;
//! ```
//!
//! (plus the collision policy: with the continuous read + non-blocking commit
//! above, a same-edge read-write to one address captures the OLD word —
//! ReadFirst, matching the simulator's default.)
//!
//! This test is the ABI's proof: the SIMULATOR runs the module against a real
//! `Memory` object, the TRANSPILED child is instantiated under the hand-written
//! owner above, and both must reproduce the trace derived from the memory
//! windows (`cycle_dataflow_memory_derivation.rs`'s m3 scenario: read latency,
//! ReadFirst same-edge collision, hold-when-unstaged, one-edge write
//! visibility).

mod common;
use common::{verilator_available, verilator_command};
use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Clock, ClockDomain, Logic, Memory};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

/// Per-invocation nonce for temporary directories. Two tests in one binary can
/// transpile or Verilate the same top module at the same moment, and a directory
/// keyed on the process id alone is then shared: one test's cleanup deletes the
/// file the other's Verilator is about to read (seen on a 96-core host,
/// 2026-09-03). Same rule as the Verilator work dir in CLAUDE.md.
static TMP_NONCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct MainClk;
impl ClockDomain for MainClk {}

const MEM_USER_SRC: &str = r#"
#[hardware(sequential)]
async fn mem_user(
    clk: Clock<MainClk>,
    addr: In<Bits<4>, MainClk>,
    din: In<Bits<8>, MainClk>,
    we: In<Logic, MainClk>,
    dout: Out<Bits<8>, MainClk>,
    m: Memory<Bits<8>, 1, 1, MainClk, 1, 1>,
) {
    let mut q: Bits<8> = Bits::zero();
    loop {
        if we.read() == Logic::One {
            m.write_port::<0>().write(addr.read().as_usize(), din.read());
        } else {
            m.read_port::<0>().read(addr.read().as_usize());
        }
        clk.tick().await;
        if m.read_port::<0>().is_ready() {
            q = m.read_port::<0>().data();
        }
        dout.write(q);
    }
}
"#;

#[hardware(sequential)]
async fn mem_user(
    clk: Clock<MainClk>,
    addr: In<Bits<4>, MainClk>,
    din: In<Bits<8>, MainClk>,
    we: In<Logic, MainClk>,
    dout: Out<Bits<8>, MainClk>,
    m: Memory<Bits<8>, 1, 1, MainClk, 1, 1>,
) {
    let mut q: Bits<8> = Bits::zero();
    loop {
        if we.read() == Logic::One {
            m.write_port::<0>().write(addr.read().as_usize(), din.read());
        } else {
            m.read_port::<0>().read(addr.read().as_usize());
        }
        clk.tick().await;
        if m.read_port::<0>().is_ready() {
            q = m.read_port::<0>().data();
        }
        dout.write(q);
    }
}

/// The owner's side of the bus — the ABI contract, hand-written. Deliberately
/// nothing but an array, a continuous read, and a guarded non-blocking commit.
const OWNER_SV: &str = r#"
module owner_top(
    input  logic clk,
    input  logic [3:0] addr,
    input  logic [7:0] din,
    input  logic we,
    output logic [7:0] dout
);
    logic [7:0] mem [0:15];
    logic [3:0] m_rd0_addr;
    logic [7:0] m_rd0_data;
    logic m_wr0_en;
    logic [3:0] m_wr0_addr;
    logic [7:0] m_wr0_data;

    mem_user #(.M_ADDR_W(4)) child (
        .clk(clk), .addr(addr), .din(din), .we(we), .dout(dout),
        .m_rd0_data(m_rd0_data), .m_rd0_addr(m_rd0_addr),
        .m_wr0_en(m_wr0_en), .m_wr0_addr(m_wr0_addr), .m_wr0_data(m_wr0_data)
    );

    assign m_rd0_data = mem[m_rd0_addr];
    always_ff @(posedge clk) if (m_wr0_en) mem[m_wr0_addr] <= m_wr0_data;

    initial begin
        for (int i = 0; i < 16; i++) mem[i] = '0;
    end
endmodule
"#;

#[test]
fn received_memory_bus_matches_the_owned_memory_semantics() {
    // The m3 scenario, drive-then-clock.
    let stim: [(u64, u64, u64); 6] = [
        // (we, addr, din) — write [5]=0xAA AND read [5]? one port set: write.
        (1, 5, 0xAA), // write [5]
        (0, 5, 0),    // read [5] → visible next obs
        (0, 5, 0),    // read again (also covers hold shape)
        (1, 7, 0x34), // write [7]
        (0, 7, 0),    // read [7]
        (0, 5, 0),    // read [5]
    ];
    // Derived from the memory windows: a read staged at edge N is observed at
    // obs N; a write at edge N is readable from edge N+1. Cycle 1 stages a
    // WRITE (no read) → dout holds its initial 0; cycle 2 reads [5] → 0xAA at
    // obs 2; …
    let derived: [u64; 6] = [0x00, 0xAA, 0xAA, 0xAA, 0x34, 0xAA];

    // ── Simulator, real Memory object ────────────────────────────────────────
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let mem = Memory::<Bits<8>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 16);
    let (addr_drv, addr_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (din_drv, din_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (we_drv, we_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (dout_out, dout_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = dout_out.dirty_handle();
    let reads = vec![addr_in.wire_id(), din_in.wire_id(), we_in.wire_id()];
    exec.spawn_wired(
        mem_user(clk.clone(), addr_in, din_in, we_in, dout_out, mem),
        vec![dh],
        reads,
    );
    let sim: Vec<u64> = stim
        .iter()
        .map(|&(we, a, d)| {
            we_drv.write(if we == 1 { Logic::One } else { Logic::Zero });
            addr_drv.write(Bits::from_usize(a as usize));
            din_drv.write(Bits::from_usize(d as usize));
            exec.tick_clock(&mut clk);
            dout_obs.read().as_u128() as u64
        })
        .collect();
    assert_eq!(sim, derived, "the SIMULATOR disagrees with the derived trace");

    // ── Transpiled child under the hand-written owner ────────────────────────
    if !verilator_available() {
        return;
    }
    let child_sv = copper_codegen::transpile_source(
        MEM_USER_SRC,
        Some("mem_user"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("transpile");

    let work = std::env::temp_dir().join(format!("copper_rmabi_{}_{}", std::process::id(), TMP_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("mem_user.sv"), &child_sv).unwrap();
    std::fs::write(work.join("owner_top.sv"), OWNER_SV).unwrap();

    let mut tb = String::from(
        "#include \"Vowner_top.h\"\n#include \"verilated.h\"\n#include <iostream>\n\
         int main(int c, char** v) { Verilated::commandArgs(c, v);\n\
         Vowner_top* t = new Vowner_top(); t->clk = 0; t->eval();\n",
    );
    for (we, a, d) in stim {
        tb.push_str(&format!(
            "t->we = {we}; t->addr = {a}; t->din = {d}; \
             t->clk=0; t->eval(); t->clk=1; t->eval(); \
             std::cout << (int)t->dout << std::endl;\n"
        ));
    }
    tb.push_str("return 0; }\n");
    std::fs::write(work.join("tb.cpp"), &tb).unwrap();

    let out = verilator_command()
        .current_dir(&work)
        .args([
            "--cc", "--exe", "--build", "--top-module", "owner_top",
            "-Wno-DECLFILENAME", "-Wno-WIDTHEXPAND", "-CFLAGS", "-std=c++14",
        ])
        .arg(work.join("owner_top.sv"))
        .arg(work.join("mem_user.sv"))
        .arg(work.join("tb.cpp"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "verilator build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new(work.join("obj_dir/Vowner_top")).output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&work);
    let hw: Vec<u64> = stdout.lines().filter_map(|l| l.trim().parse().ok()).collect();
    assert_eq!(
        hw, derived,
        "the TRANSPILED child under the hand-written OWNER disagrees with the \
         derived trace — the bus ABI's timing contract is broken on one side"
    );
}
