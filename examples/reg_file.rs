// Register file using Memory<u8, 2, 1, D> — 4 registers x 8-bit.
//
// Two independent read ports (rs1, rs2) let two operands be fetched in the same
// cycle, like a CPU register file. One write port (wr_addr/wr_data/we) commits
// at the clock edge. Read-first: rd1/rd2 reflect old values when reading and
// writing the same register in the same cycle.
// Cross-validated against verilog/reg_file.v via Verilator.
//
// Timing pattern: local out1/out2 are computed with rp.read() in poll2 after tick.
// rp.read() triggers sync_if_advanced, committing the previous cycle's staged write
// before returning data[addr]. This gives correct read-first semantics.

use copper_core::{Bit, Clock, ClockDomain, Logic, Memory};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareTest};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware]
async fn reg_file(
    clk: Clock<MainClk>,
    we:      Arc<Mutex<Bit>>,
    wr_addr: Arc<Mutex<u8>>,
    wr_data: Arc<Mutex<u8>>,
    rs1:     Arc<Mutex<u8>>,
    rs2:     Arc<Mutex<u8>>,
    rd1:     Arc<Mutex<u8>>,
    rd2:     Arc<Mutex<u8>>,
) {
    let mem = Memory::<u8, 2, 1, MainClk>::new(clk.clone(), 4);
    let mut out1: u8 = 0;
    let mut out2: u8 = 0;
    loop {
        // Emit registered outputs (from previous edge) before waiting for the next edge.
        *rd1.lock().unwrap() = out1;
        *rd2.lock().unwrap() = out2;

        clk.tick().await;

        // In poll2: read inputs and compute next outputs.
        // rp.read() triggers sync_if_advanced, committing the previous cycle's
        // staged write before reading. This cycle's write is staged below.
        let a1 = (*rs1.lock().unwrap() & 0x3) as usize;
        let a2 = (*rs2.lock().unwrap() & 0x3) as usize;

        out1 = mem.read_port::<0>().read(a1); // sync fires here; prev write commits
        out2 = mem.read_port::<1>().read(a2); // sync already done for this cycle

        if *we.lock().unwrap() == Bit::ONE {
            let wa = (*wr_addr.lock().unwrap() & 0x3) as usize;
            mem.write_port::<0>().write(wa, *wr_data.lock().unwrap());
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn bits8(v: u8) -> Vec<Logic> {
    (0..8).map(|i| if (v >> i) & 1 == 1 { Logic::One } else { Logic::Zero }).collect()
}

fn bits2(v: u8) -> Vec<Logic> {
    (0..2).map(|i| if (v >> i) & 1 == 1 { Logic::One } else { Logic::Zero }).collect()
}

// ── main ─────────────────────────────────────────────────────────────────────

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let we      = Arc::new(Mutex::new(Bit::ZERO));
    let wr_addr = Arc::new(Mutex::new(0u8));
    let wr_data = Arc::new(Mutex::new(0u8));
    let rs1     = Arc::new(Mutex::new(0u8));
    let rs2     = Arc::new(Mutex::new(0u8));
    let rd1     = Arc::new(Mutex::new(0u8));
    let rd2     = Arc::new(Mutex::new(0u8));

    exec.spawn(reg_file(
        clk.clone(),
        Arc::clone(&we),
        Arc::clone(&wr_addr),
        Arc::clone(&wr_data),
        Arc::clone(&rs1),
        Arc::clone(&rs2),
        Arc::clone(&rd1),
        Arc::clone(&rd2),
    ));

    let mut test = HardwareTest::new("reg_file")
        .with_verilog("verilog/reg_file.v")
        .with_waveform("waveforms/reg_file.vcd");

    // Stimulus: (we, wr_addr, wr_data, rs1, rs2, expected_rd1, expected_rd2)
    //
    // Cycles 1-4: load each register (write-only reads; rs1=rs2=0 throughout).
    //   Read-first: rd1/rd2 show old values until the next cycle.
    //
    // Cycles 5-6: read pairs of registers simultaneously.
    //   Both read ports active in the same cycle — key register-file feature.
    //
    // Cycles 7-8: simultaneous write+read of reg 1 (read-first → old value in cycle 7,
    //   new value visible in cycle 8).
    let stimulus: &[(Bit, u8, u8, u8, u8, u8, u8)] = &[
        // we  wr_addr  wr_data  rs1  rs2  exp_rd1  exp_rd2
        (Bit::ONE,  0, 0x10,  0, 0,  0x00, 0x00), // load reg0=0x10; rd still X→0
        (Bit::ONE,  1, 0x20,  0, 0,  0x10, 0x10), // load reg1=0x20; rd0 now 0x10
        (Bit::ONE,  2, 0x30,  0, 0,  0x10, 0x10), // load reg2=0x30
        (Bit::ONE,  3, 0x40,  0, 0,  0x10, 0x10), // load reg3=0x40
        (Bit::ZERO, 0, 0x00,  0, 3,  0x10, 0x40), // read reg0 + reg3 simultaneously
        (Bit::ZERO, 0, 0x00,  1, 2,  0x20, 0x30), // read reg1 + reg2 simultaneously
        (Bit::ONE,  1, 0xFF,  0, 1,  0x10, 0x20), // write reg1=0xFF, read reg0+reg1: read-first
        (Bit::ZERO, 0, 0x00,  0, 1,  0x10, 0xFF), // read reg0+reg1: reg1 now 0xFF
    ];

    println!("reg_file: we wr_addr wr_data rs1 rs2 | rd1(got/exp)  rd2(got/exp)  pass?");
    for (cycle, &(w, wa, wd, a1, a2, exp1, exp2)) in stimulus.iter().enumerate() {
        *we.lock().unwrap()      = w;
        *wr_addr.lock().unwrap() = wa;
        *wr_data.lock().unwrap() = wd;
        *rs1.lock().unwrap()     = a1;
        *rs2.lock().unwrap()     = a2;

        exec.tick_clock(&mut clk);

        let out1 = *rd1.lock().unwrap();
        let out2 = *rd2.lock().unwrap();
        let pass = if out1 == exp1 && out2 == exp2 { "PASS" } else { "FAIL" };

        println!("  {:>2}  {}   {}       0x{:02X}   {}   {} | 0x{:02X}/0x{:02X}  0x{:02X}/0x{:02X}  {}",
            cycle + 1,
            if w == Bit::ONE { 1 } else { 0 }, wa, wd, a1, a2,
            out1, exp1, out2, exp2, pass);

        test.record_cycle(
            cycle,
            &[
                ("we",      &[if w == Bit::ONE { Logic::One } else { Logic::Zero }]),
                ("wr_addr", &bits2(wa)),
                ("wr_data", &bits8(wd)),
                ("rs1",     &bits2(a1)),
                ("rs2",     &bits2(a2)),
            ],
            &[
                ("rd1", &bits8(out1)),
                ("rd2", &bits8(out2)),
            ],
        );
    }

    test.finish().assert_passed();
}
