// Copper re-implementation of BaseJump STL `bsg_dff_en` (an enabled register),
// checked for cycle-level equivalence against the original BaseJump Verilog
// (examples/basejump/sv/bsg_dff_en.sv) under Verilator. This anchors Copper's
// simulator to an independent, third-party hardware reference — not to Copper's
// own transpiler — which is what makes the same-source sim≡synth claim a hardware
// property rather than a self-consistency check.

use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::{make_cycle, HardwareExecutor, HardwareTest, SimulationTrace};

struct MainClk;
impl ClockDomain for MainClk {}

// Enabled register: on each clock edge, capture `data_i` when `en_i` is high;
// hold otherwise. Mirrors `always_ff @(posedge clk) if (en_i) data_r <= data_i;`.
#[hardware(sequential)]
async fn bsg_dff_en(
    clk: Clock<MainClk>,
    data_i: In<Bits<8>, MainClk>,
    en_i: In<Logic, MainClk>,
    data_o: Out<Bits<8>, MainClk>,
) {
    loop {
        clk.tick().await;
        if en_i.read() == Logic::One {
            data_o.write(data_i.read());
        }
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (data_drv, data_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (en_drv, en_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(bsg_dff_en(clk.clone(), data_in, en_in, out_drv), vec![dh]);

    let mut test = HardwareTest::new("bsg_dff_en")
        .with_verilog("examples/basejump/sv/bsg_dff_en.sv")
        .with_waveform("waveforms/bsg_dff_en.vcd");

    // Deterministic pseudo-random (LCG) stimulus over many cycles, plus a few
    // forced patterns first (capture, hold across en=0, back-to-back updates) to
    // guarantee those cases are covered regardless of the RNG.
    let forced: &[(bool, u8)] = &[
        (true, 0x11),  // capture 0x11
        (false, 0x22), // hold 0x11
        (false, 0x33), // hold 0x11
        (true, 0x44),  // capture 0x44
        (true, 0x55),  // capture 0x55
        (false, 0x66), // hold 0x55
        (true, 0x77),  // capture 0x77
    ];
    const N_RANDOM: usize = 64;
    let mut rng: u32 = 0x1234_5678;
    let stimulus: Vec<(bool, u8)> = forced
        .iter()
        .copied()
        .chain((0..N_RANDOM).map(|_| {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            (((rng >> 3) & 1) == 1, (rng >> 8) as u8) // ~50% enable, full data range
        }))
        .collect();

    let mut model = 0u8; // Verilator powers `data_r` up to 0; Copper reg starts 0.
    let mut expected_cycles = Vec::new();
    for (i, &(en, data)) in stimulus.iter().enumerate() {
        data_drv.write(Bits::from_u8(data));
        en_drv.write(Logic::from_bool(en));
        exec.tick_clock(&mut clk);

        if en {
            model = data;
        }

        let en_l = Logic::from_bool(en);
        let data_b = Bits::<8>::from_u8(data);
        let exp_b = Bits::<8>::from_u8(model);
        let out = out_obs.read();

        test.record_cycle(
            i,
            &[("data_i", &data_b[..]), ("en_i", std::slice::from_ref(&en_l))],
            &[("data_o", &out[..])],
        );
        expected_cycles.push(make_cycle(
            i,
            &[("data_i", &data_b[..]), ("en_i", std::slice::from_ref(&en_l))],
            &[("data_o", &exp_b[..])],
        ));
    }

    let expected = SimulationTrace::from_cycles(expected_cycles);
    test.finish_with_expected(&expected).assert_passed();
    println!("bsg_dff_en: Copper sim ≡ reference model ≡ BaseJump Verilog (Verilator) ✓");
}
