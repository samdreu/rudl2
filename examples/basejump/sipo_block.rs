// Mid-phase-read investigation, anchored to independent BaseJump hardware.
//
// A block serial-in-parallel-out deserializer: assemble 4 input words (read one
// per cycle) and present them as a 16-bit parallel word. The natural Copper coding
// reads `data_i` at the loop top (word 0) AND after each `clk.tick().await`
// (words 1,2,3 — "mid-phase" reads). If the simulator samples those mid-phase
// reads on the wrong cycle, the assembled word diverges from the BaseJump-derived
// reference sipo_block.sv, which reads word i on the i-th cycle of each block.
//
// This is the accum_2 mid-phase-read question with a third-party hardware golden.

use copper_core::port::{registered_wire, wire, In, Out, RegOut};
use copper_core::{Bits, Logic};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareTest};

struct MainClk;
impl copper_core::ClockDomain for MainClk {}

// Assemble 4 words (4 bits each) → 16 bits: {w3,w2,w1,w0}, w0 in the low nibble.
// `data_o` is a write-before-tick Moore output — the independent reference
// (sipo_block.sv) registers it in an `always_ff`, so this needs `RegOut`, not
// plain `Out` (which would present the packed word a cycle early, combinationally,
// right when `.write()` runs instead of at the following edge).
//
// `w0_dbg`..`w3_dbg` are debug-only output ports (not present in the reference
// design) that mirror each internal `wN` the instant it's captured, so the
// waveform can show the exact same internal state the .sv reference already
// exposes (it has `w0`/`w1`/`w2` as real registers) for direct comparison.
#[hardware(sequential)]
async fn sipo_block(
    clk: copper_core::Clock<MainClk>,
    data_i: In<Bits<4>, MainClk>,
    data_o: RegOut<Bits<16>, MainClk>,
    w0_dbg: Out<Bits<4>, MainClk>,
    w1_dbg: Out<Bits<4>, MainClk>,
    w2_dbg: Out<Bits<4>, MainClk>,
    w3_dbg: Out<Bits<4>, MainClk>,
) {
    loop {
        let w0 = data_i.read();
        w0_dbg.write(w0.clone());
        clk.tick().await;
        let w1 = data_i.read(); // mid-phase
        w1_dbg.write(w1.clone());
        clk.tick().await;
        let w2 = data_i.read(); // mid-phase
        w2_dbg.write(w2.clone());
        clk.tick().await;
        let w3 = data_i.read(); // mid-phase
        w3_dbg.write(w3.clone());
        let mut bits = [Logic::Zero; 16];
        for k in 0..4 {
            bits[k] = w0[k];
            bits[4 + k] = w1[k];
            bits[8 + k] = w2[k];
            bits[12 + k] = w3[k];
        }
        data_o.write(Bits::from_slice(&bits));
        clk.tick().await;
    }
}

fn main() {
    let mut clk = copper_core::Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (d_drv, d_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (o_drv, o_obs) = registered_wire::<Bits<16>, MainClk>(&clk, Bits::zero());
    let (w0_drv, w0_obs) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (w1_drv, w1_obs) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (w2_drv, w2_obs) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (w3_drv, w3_obs) = wire::<Bits<4>, MainClk>(Bits::zero());
    let dhs = vec![
        o_drv.dirty_handle(),
        w0_drv.dirty_handle(),
        w1_drv.dirty_handle(),
        w2_drv.dirty_handle(),
        w3_drv.dirty_handle(),
    ];
    exec.spawn_wired(
        sipo_block(clk.clone(), d_in, o_drv, w0_drv, w1_drv, w2_drv, w3_drv),
        dhs,
    );

    // Golden = the independent Verilog reference only (no separate hand-derived
    // expected trace) — `finish()` (not `finish_with_expected`) skips the
    // trace-vs-golden comparison and only runs the Verilator cross-check, which
    // replays these same driven inputs into `sipo_block.sv` and compares ITS
    // outputs against what the Copper sim produced. Both sides get a waveform
    // (`waveforms/sipo_block.vcd` = Copper sim, phased so it has the same
    // two-timestamps-per-cycle shape as Verilator's negedge/posedge dumps, with an
    // explicit `clk` signal for direct visual comparison; `sipo_block_verilator.vcd`
    // = Verilator/Verilog, with full internal register visibility).
    let mut test = HardwareTest::new("sipo_block")
        .with_verilog("examples/basejump/sv/sipo_block.sv")
        .with_phased_waveform("waveforms/sipo_block.vcd")
        .with_verilator_waveform("waveforms/sipo_block_verilator.vcd");

    // Distinct stream so each cycle's word is identifiable (1..=12, wrapping nibble).
    let stream: Vec<u8> = (0..12).map(|i| ((i % 15) + 1) as u8).collect();

    let mut prev_out = Bits::<16>::zero();
    let mut prev_w = [Bits::<4>::zero(), Bits::<4>::zero(), Bits::<4>::zero(), Bits::<4>::zero()];
    for (cyc, &d) in stream.iter().enumerate() {
        let dv = Bits::<4>::from_usize((d as usize) & 0xF);
        d_drv.write(dv);
        exec.tick_clock(&mut clk);

        let out = o_obs.read();
        let w = [w0_obs.read(), w1_obs.read(), w2_obs.read(), w3_obs.read()];
        // pre-edge snapshot (clk=0) holds each signal's value from BEFORE this
        // tick_clock call — a debug port only changes when its `wN_dbg.write()`
        // actually runs, so "before this cycle" is just "whatever it held at the
        // end of the previous cycle." `record_cycle_phased` auto-generates the
        // `clk` trace itself (0 at the pre-edge timestamp, 1 at post-edge).
        test.record_cycle_phased(
            cyc,
            &[("data_i", dv.as_array())],
            &[("data_o", prev_out.as_array())],
            &[("data_o", out.as_array())],
        );
        // Debug-only internal probes (w0_dbg..w3_dbg) — VCD visibility only, not
        // fed to the Verilator cross-check (see `add_debug_signals_phased`).
        test.add_debug_signals_phased(
            cyc,
            &[
                ("w0", prev_w[0].as_array()),
                ("w1", prev_w[1].as_array()),
                ("w2", prev_w[2].as_array()),
                ("w3", prev_w[3].as_array()),
            ],
            &[
                ("w0", w[0].as_array()),
                ("w1", w[1].as_array()),
                ("w2", w[2].as_array()),
                ("w3", w[3].as_array()),
            ],
        );
        prev_out = out;
        prev_w = w;
    }

    let result = test.finish();
    println!("sipo_block: verilator={:?}", result.verilator_ok);
    result.assert_passed();
}
