use copper_core::types::{Bits, Logic, Clock, ClockDomain};
use copper_core::port::{In, Out, wire};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(sequential)]
async fn shift_register <const N: usize, const N_1: usize> (
    d: In<Logic, MainClk>,
    clk: Clock<MainClk>,
    en: In<Logic, MainClk>,
    dir: In<Logic, MainClk>,
    rstn: In<Logic, MainClk>,
    out: Out<Bits<N>, MainClk>,
) {
    const { assert!(N-1 == N_1, "N_1 must equal N-1") };
    let mut out_n = Bits::x();

    loop {
        if !rstn.read().as_bool() {
            out_n = Bits::zero();
        } else if en.read().as_bool() {
            let mut shifted = Bits::zero();
            if dir.read() == Logic::Zero {
                // shift left
                shifted[0] = d.read();
                for i in 1..N {
                    shifted[i] = out_n[i-1];
                }
            } else {
                // shift right
                shifted[N_1] = d.read();
                for i in 0..N_1 {
                    shifted[i] = out_n[i+1];
                }
            }
            out_n = shifted;
        }

        out.write(out_n);
        clk.tick().await;
    }
}

fn main() {
    const N: usize = 8;
    const N_1: usize = 7;

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (d_drv, d_in)       = wire::<Logic, MainClk>(Logic::Zero);
    let (en_drv, en_in)     = wire::<Logic, MainClk>(Logic::Zero);
    let (dir_drv, dir_in)   = wire::<Logic, MainClk>(Logic::Zero);
    let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
    let (out_out, out_obs)  = wire::<Bits<N>, MainClk>(Bits::zero());

    let dh = out_out.dirty_handle();
    let reads = vec![d_in.wire_id(), en_in.wire_id(), dir_in.wire_id(), rstn_in.wire_id()];
    exec.spawn_wired(
        shift_register::<N, N_1>(d_in, clk.clone(), en_in, dir_in, rstn_in, out_out),
        vec![dh],
        reads,
    );

    let mut test = HardwareTest::new("shift_reg")
        .with_verilog("examples/sequential/sv/shift_register.sv")
        .with_waveform("waveforms/shift_register.vcd");

    let z = Logic::Zero;
    let o = Logic::One;

    // (rstn, en, dir, d)
    let cases: &[(Logic, Logic, Logic, Logic)] = &[
        (z, z, z, z), // cycle 0: reset
        (o, o, z, o), // cycle 1: left shift, d=1
        (o, o, z, o), // cycle 2: left shift, d=1
        (o, o, z, z), // cycle 3: left shift, d=0
        (o, z, z, z), // cycle 4: hold (en=0)
        (o, o, o, o), // cycle 5: right shift, d=1
        (o, o, o, z), // cycle 6: right shift, d=0
        (z, o, o, o), // cycle 7: reset overrides en/dir/d
    ];

    // Reference model expressed in plain u8 shifts — this is the behavioral
    // spec (matches the SV always-block), independent of how the DUT
    // represents/indexes bits internally. Expected values are derived from
    // this, not hand-traced against the DUT's implementation.
    fn next_state(prev: u8, rstn: Logic, en: Logic, dir: Logic, d: Logic) -> u8 {
        if rstn == Logic::Zero {
            0
        } else if en != Logic::One {
            prev
        } else if dir == Logic::Zero {
            (prev << 1) | (d == Logic::One) as u8 // shift left, d enters LSB
        } else {
            (prev >> 1) | (((d == Logic::One) as u8) << (N - 1)) // shift right, d enters MSB
        }
    }

    let mut state = 0u8;
    let expected_out: Vec<Bits<N>> = cases.iter().map(|&(rstn, en, dir, d)| {
        state = next_state(state, rstn, en, dir, d);
        Bits::from_u8(state)
    }).collect();

    for (i, &(rstn, en, dir, d)) in cases.iter().enumerate() {
        rstn_drv.write(rstn);
        en_drv.write(en);
        dir_drv.write(dir);
        d_drv.write(d);

        exec.tick_clock(&mut clk);

        test.record_cycle(
            i,
            &[("rstn", &[rstn]), ("en", &[en]), ("dir", &[dir]), ("d", &[d])],
            &[("out", out_obs.read().as_array())],
        );
    }

    let expected = SimulationTrace::from_cycles(
        cases.iter().zip(expected_out.iter()).enumerate().map(|(i, (&(rstn, en, dir, d), &out))| {
            make_cycle(
                i,
                &[("rstn", &[rstn]), ("en", &[en]), ("dir", &[dir]), ("d", &[d])],
                &[("out", out.as_array())],
            )
        }).collect(),
    );

    test.finish_with_expected(&expected).assert_passed();
}
