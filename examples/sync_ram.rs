// Synchronous single-port RAM using Memory<u8, 1, 1, D>.
//
// Demonstrates read-first block RAM semantics: when a write and read hit the
// same address in the same cycle, data_out captures the OLD value (before the
// write commits). Cross-validated against verilog/sync_ram.v via Verilator.
//
// Timing pattern: emit local dout in poll1, then in poll2 (after tick) read
// and write through the memory ports. Clock edges commit staged writes.

use copper_core::{Bit, Clock, ClockDomain, Logic, Memory};
use copper_macros::hardware;
use copper_sim::{emit, HardwareExecutor, HardwareTest};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(function_typed)]
async fn sync_ram(
    clk: Clock<MainClk>,
    addr: Arc<Mutex<u8>>,
    we: Arc<Mutex<Bit>>,
    data_in: Arc<Mutex<u8>>,
) -> u8 {
    let mem = Memory::<u8, 1, 1, MainClk>::new(clk.clone(), 4);
    let mut dout: u8 = 0;
    loop {
        emit!(dout);
        clk.tick().await;

        let a   = (*addr.lock().unwrap() & 0x3) as usize;
        let w   = *we.lock().unwrap();
        let din = *data_in.lock().unwrap();

        // Read the committed memory state for this cycle.
        dout = mem.read_port::<0>().read(a);

        if w == Bit::ONE {
            mem.write_port::<0>().write(a, din);
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

    let addr    = Arc::new(Mutex::new(0u8));
    let we      = Arc::new(Mutex::new(Bit::ZERO));
    let data_in = Arc::new(Mutex::new(0u8));

    let data_out = exec.spawn_function_typed(
        0u8,
        sync_ram(clk.clone(), Arc::clone(&addr), Arc::clone(&we), Arc::clone(&data_in)),
    );

    let mut test = HardwareTest::new("sync_ram")
        .with_verilog("verilog/sync_ram.v")
        .with_waveform("waveforms/sync_ram.vcd");

    // Stimulus: address, write-enable, data-in, expected data-out after the edge.
    //
    // Cycles 1-3: write 0x11/0x22/0x33 to addrs 0/1/2.
    //   Output is read-first: data_out = old value (0) before the write commits.
    //
    // Cycles 4-6: read back each address with WE=0.
    //   Output now reflects the committed writes.
    //
    // Cycle 7: simultaneous write+read at addr 0 (write 0xAA, read same addr).
    //   Read-first → data_out = 0x11 (old), not 0xAA.
    //
    // Cycle 8: read addr 0 again.
    //   0xAA is now committed → data_out = 0xAA.
    let stimulus: &[(u8, Bit, u8, u8)] = &[
        // addr, we, data_in, expected data_out
        (0, Bit::ONE,  0x11, 0x00), // write 0x11 → addr 0; read-first captures 0x00
        (1, Bit::ONE,  0x22, 0x00), // write 0x22 → addr 1; old value 0x00
        (2, Bit::ONE,  0x33, 0x00), // write 0x33 → addr 2; old value 0x00
        (0, Bit::ZERO, 0x00, 0x11), // read addr 0; committed 0x11
        (1, Bit::ZERO, 0x00, 0x22), // read addr 1
        (2, Bit::ZERO, 0x00, 0x33), // read addr 2
        (0, Bit::ONE,  0xAA, 0x11), // write 0xAA + read addr 0; read-first → 0x11
        (0, Bit::ZERO, 0x00, 0xAA), // read addr 0; 0xAA now committed
    ];

    println!("sync_ram: addr  we  data_in  data_out  expected  pass?");
    for (cycle, &(a, w, din, expected)) in stimulus.iter().enumerate() {
        *addr.lock().unwrap()    = a;
        *we.lock().unwrap()      = w;
        *data_in.lock().unwrap() = din;

        exec.tick_clock(&mut clk);

        let dout = *data_out.lock().unwrap();
        let pass = if dout == expected { "PASS" } else { "FAIL" };
        println!("  {:>2}     {}     {}   0x{:02X}    0x{:02X}      0x{:02X}    {}",
            cycle + 1, a,
            if w == Bit::ONE { 1 } else { 0 },
            din, dout, expected, pass);

        test.record_cycle(
            cycle,
            &[
                ("addr",    &bits2(a)),
                ("we",      &[if w == Bit::ONE { Logic::One } else { Logic::Zero }]),
                ("data_in", &bits8(din)),
            ],
            &[("data_out", &bits8(dout))],
        );
    }

    test.finish().assert_passed();
}
