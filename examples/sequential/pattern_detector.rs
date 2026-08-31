use copper_core::types::{Bits, Logic, Clock, ClockDomain};
use copper_core::port::{In, Out, wire};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};

struct MainClk;
impl ClockDomain for MainClk {}

#[derive(PartialEq)]
enum State {
    IDLE = 0,
    S1 = 1,
    S11 = 2,
    S110 = 3,
    S1101 = 4,
    S11010 = 5,
    S110101 = 6,
}

#[hardware(sequential)]
async fn det_110101 (
    clk: Clock<MainClk>,
    rstn: In<Logic, MainClk>,
    in_i: In<Logic, MainClk>,
    out_o: Out<Logic, MainClk>,
) {
    let mut state = State::IDLE;
    loop {
        if rstn.read() == Logic::Zero {
            state = State::IDLE;
        } else {
            state = match (state, in_i.read()) {
                (State::IDLE, Logic::One) => State::S1,
                (State::S1, Logic::One) => State::S11,
                (State::S11, Logic::Zero) => State::S110,
                (State::S110, Logic::One) => State::S1101,
                (State::S1101, Logic::Zero) => State::S11010,
                (State::S11010, Logic::One) => State::S110101,
                _ => State::IDLE,
            };
        }

        if state == State::S110101 {
            out_o.write(Logic::One);
        } else {
            out_o.write(Logic::Zero);
        }

        clk.tick().await;
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
    let (in_drv,   in_in)   = wire::<Logic, MainClk>(Logic::Zero);
    let (out_out,  out_obs) = wire::<Logic, MainClk>(Logic::Zero);

    let dh = out_out.dirty_handle();
    let reads = vec![rstn_in.wire_id(), in_in.wire_id()];
    exec.spawn_wired(
        det_110101(clk.clone(), rstn_in, in_in, out_out),
        vec![dh],
        reads,
    );

    let mut test = HardwareTest::new("det_110101")
        .with_verilog("examples/sequential/sv/pattern_detector.sv")
        .with_waveform("waveforms/pattern_detector.vcd")
        .with_verilator_waveform("waveforms/pattern_detector_verilator.vcd"); // dumps all SV internals


    let z = Logic::Zero;
    let o = Logic::One;

    // (rstn, in_bit)
    let cases: &[(Logic, Logic)] = &[
        (z, z), // cycle 0: reset
        (o, o), // cycle 1: '1' → IDLE→S1
        (o, o), // cycle 2: '1' → S1→S11
        (o, z), // cycle 3: '0' → S11→S110
        (o, o), // cycle 4: '1' → S110→S1101
        (o, z), // cycle 5: '0' → S1101→S11010
        (o, o), // cycle 6: '1' → S11010→S110101  DETECT
        (o, o), // cycle 7: '1' → S110101→S1 (overlap)
        (o, z), // cycle 8: '0' → S1→IDLE
        (z, o), // cycle 9: reset overrides input
    ];

    // Reference model — u8 state encoding matching the SV spec,
    // independent of how the DUT's State enum works internally.
    fn next_ref(state: u8, rstn: Logic, in_val: Logic) -> u8 {
        if rstn == Logic::Zero { return 0; }
        match (state, in_val) {
            (0, Logic::One)  => 1, // IDLE    → S1
            (1, Logic::One)  => 2, // S1      → S11
            (2, Logic::Zero) => 3, // S11     → S110
            (3, Logic::One)  => 4, // S110    → S1101
            (4, Logic::Zero) => 5, // S1101   → S11010
            (5, Logic::One)  => 6, // S11010  → S110101
            (6, Logic::One)  => 1, // S110101 → S1 (overlap)
            _                => 0, // → IDLE
        }
    }

    let mut ref_state = 0u8;
    let expected_out: Vec<Logic> = cases.iter().map(|&(rstn, in_val)| {
        ref_state = next_ref(ref_state, rstn, in_val);
        Logic::from_bool(ref_state == 6)
    }).collect();

    for (i, &(rstn, in_val)) in cases.iter().enumerate() {
        rstn_drv.write(rstn);
        in_drv.write(in_val);
        exec.tick_clock(&mut clk);
        test.record_cycle(
            i,
            &[("rstn", &[rstn]), ("in", &[in_val])],
            &[("out", &[out_obs.read()])],
        );
    }

    let expected = SimulationTrace::from_cycles(
        cases.iter().zip(expected_out.iter()).enumerate().map(|(i, (&(rstn, in_val), &exp))| {
            make_cycle(
                i,
                &[("rstn", &[rstn]), ("in", &[in_val])],
                &[("out", &[exp])],
            )
        }).collect(),
    );

    test.finish_with_expected(&expected).assert_passed();
}
