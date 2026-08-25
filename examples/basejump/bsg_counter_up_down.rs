// Copper re-implementation of BaseJump STL `bsg_counter_up_down`, checked for
// cycle-level equivalence against the original BaseJump Verilog under Verilator
// (examples/basejump/sv/bsg_counter_up_down.sv). A registered counter that adds
// `up_i` and subtracts `down_i` each cycle (with wrap-around, no saturation), or
// loads `init_val_p` on `reset_i`.
//
// Parameters match BaseJump's own testbench testing/bsg_misc/bsg_counter_up_down/
// test_bsg.sv: max_val_p=7 (so count_o is 3 bits), init_val_p=0, max_step_p=1
// (up_i/down_i are 1 bit). BaseJump's testbench walks a full up/down triangle
// (init->max, max->init); we drive that triangle and additionally exercise the
// wrap and simultaneous up+down edge cases the DUT supports. The golden model is
// the DUT's own recurrence, `count <= count - down + up` (mod 8).

use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::{make_cycle, HardwareExecutor, HardwareTest, SimulationTrace};

struct MainClk;
impl ClockDomain for MainClk {}

const PTR_W: usize = 3; // BSG_WIDTH(max_val_p=7)
const MOD: usize = 1 << PTR_W; // 8

// count_o <= reset ? init_val_p : (count_o - down_i + up_i), registered.
#[hardware(sequential)]
async fn bsg_counter_up_down(
    clk: Clock<MainClk>,
    reset_i: In<Logic, MainClk>,
    up_i: In<Logic, MainClk>,
    down_i: In<Logic, MainClk>,
    count_o: Out<Bits<PTR_W>, MainClk>,
) {
    // `Bits<PTR_W>`, not a bare `usize`: a `usize` local is a 32-bit signal, so
    // driving the 3-bit `count_o` from it is a width truncation Verilator rejects
    // under `-Wall`. Typing the counter at its real width also removes the
    // `+ MOD … % MOD` dance — that existed only to keep a `usize` from
    // underflowing, and `Bits<3>` wraps on its own, which is what the hardware
    // does. The recurrence below is now BaseJump's own: `count_o - down_i + up_i`.
    let mut count = Bits::<PTR_W>::zero(); // init_val_p
    loop {
        clk.tick().await;
        if reset_i.read() == Logic::One {
            count = Bits::zero();
        } else {
            // NOTE `Bits::one()` is ALL ONES (7 here), not the value 1 — use a
            // literal for the step.
            let step: Bits<PTR_W> = Bits::from_lit::<1>();
            let up: Bits<PTR_W> = if up_i.read() == Logic::One { step } else { Bits::zero() };
            let down: Bits<PTR_W> = if down_i.read() == Logic::One { step } else { Bits::zero() };
            count = count - down + up; // 3-bit wrap-around, no saturation
        }
        count_o.write(count);
    }
}

// `#[cfg(not(test))]` so `tests/` can `include!` this file for its own
// harness without pulling in a second `main` (same structure as sipo_block).
#[cfg(not(test))]
fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (rst_drv, rst_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (up_drv, up_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (dn_drv, dn_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_drv, out_obs) = wire::<Bits<PTR_W>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    let reads = vec![rst_in.wire_id(), up_in.wire_id(), dn_in.wire_id()];
    exec.spawn_wired(
        bsg_counter_up_down(clk.clone(), rst_in, up_in, dn_in, out_drv),
        vec![dh],
        reads,
    );

    let mut test = HardwareTest::new("bsg_counter_up_down")
        .with_verilog("examples/basejump/sv/bsg_counter_up_down.sv")
        .with_waveform("waveforms/bsg_counter_up_down.vcd");

    // Forced (reset, up, down): up-triangle 0->7, overflow wrap, down-triangle
    // 7->0, underflow wrap, simultaneous up+down (net 0), mid-stream reset — the
    // named edge cases. Then a long deterministic pseudo-random tail for coverage.
    let forced: &[(bool, bool, bool)] = &[
        (true, false, false),  // reset -> 0
        (false, true, false),  // 0 -> 1
        (false, true, false),  // 1 -> 2
        (false, true, false),  // 2 -> 3
        (false, true, false),  // 3 -> 4
        (false, true, false),  // 4 -> 5
        (false, true, false),  // 5 -> 6
        (false, true, false),  // 6 -> 7  (max)
        (false, true, false),  // 7 -> 0  (overflow wrap)
        (false, false, true),  // 0 -> 7  (underflow wrap)
        (false, false, true),  // 7 -> 6
        (false, false, true),  // 6 -> 5
        (false, true, true),   // up+down -> net 0 (stays 5)
        (false, false, true),  // 5 -> 4
        (true, false, false),  // reset -> 0
        (false, true, false),  // 0 -> 1
    ];
    const N_RANDOM: usize = 64;
    let mut rng: u32 = 0x0bad_c0de;
    let cases: Vec<(bool, bool, bool)> = forced
        .iter()
        .copied()
        .chain((0..N_RANDOM).map(|_| {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let reset = (rng & 0xf) == 0; // ~1/16 reset
            let up = (rng >> 4) & 1 == 1;
            let down = (rng >> 5) & 1 == 1;
            (reset, up, down)
        }))
        .collect();

    // Golden model: mirror the DUT recurrence exactly.
    let mut model: usize = 0;
    let mut expected_cycles = Vec::new();
    for (i, &(rst, up, dn)) in cases.iter().enumerate() {
        rst_drv.write(Logic::from_bool(rst));
        up_drv.write(Logic::from_bool(up));
        dn_drv.write(Logic::from_bool(dn));
        exec.tick_clock(&mut clk);

        if rst {
            model = 0;
        } else {
            model = (model + MOD - dn as usize + up as usize) % MOD;
        }

        let rst_l = Logic::from_bool(rst);
        let up_l = Logic::from_bool(up);
        let dn_l = Logic::from_bool(dn);
        let out = out_obs.read();
        test.record_cycle(
            i,
            &[
                ("reset_i", std::slice::from_ref(&rst_l)),
                ("up_i", std::slice::from_ref(&up_l)),
                ("down_i", std::slice::from_ref(&dn_l)),
            ],
            &[("count_o", out.as_array())],
        );
        expected_cycles.push(make_cycle(
            i,
            &[
                ("reset_i", std::slice::from_ref(&rst_l)),
                ("up_i", std::slice::from_ref(&up_l)),
                ("down_i", std::slice::from_ref(&dn_l)),
            ],
            &[("count_o", &Bits::<PTR_W>::from_usize(model).as_array()[..])],
        ));
    }

    let expected = SimulationTrace::from_cycles(expected_cycles);
    test.finish_with_expected(&expected).assert_passed();
    println!("bsg_counter_up_down: Copper sim ≡ golden recurrence ≡ BaseJump Verilog (Verilator) ✓");
}
