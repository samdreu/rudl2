use copper_core::types::{Bits, Logic, Clock, ClockDomain};
use copper_core::port::{In, Out, wire};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};

struct MainClk;
impl ClockDomain for MainClk {}

// assuming width of 32 and initial value of 1
#[hardware(sequential)]
async fn lfsr (
    clk: Clock<MainClk>,
    reset_i: In<Logic, MainClk>,
    yumi_i: In<Logic, MainClk>,
    o: Out<Bits<32>, MainClk>,
) {
    let xor_mask = Bits::from_u32((1 << 31) | (1 << 29) | (1 << 26) | (1 << 25));
    let mut state = Bits::from_u32(1);
    loop {
        if reset_i.read().as_bool() {
            state = Bits::from_u32(1);
        } else if yumi_i.read().as_bool() {
            state = if state[0] == Logic::One {
                (state >> 1) ^ xor_mask
            } else {
                state >> 1
            };
        }

        o.write(state);
        clk.tick().await;
    }    
}


fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (reset_i_drv, reset_i_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (yumi_i_drv, yumi_i_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (o_out, o_obs) = wire::<Bits<32>, MainClk>(Bits::from_u32(1));

    let dh = o_out.dirty_handle();
    let reads = vec![reset_i_in.wire_id(), yumi_i_in.wire_id()];
    exec.spawn_wired(
        lfsr(clk.clone(), reset_i_in, yumi_i_in, o_out),
        vec![dh],
        reads,
    );

    let mut test = HardwareTest::new("lfsr")
        .with_verilog("examples/sequential/sv/lfsr.sv")
        .with_waveform("waveforms/lfsr.vcd");

    // Reference model — plain u32 arithmetic, independent of Bits<N>/Logic
    fn next_lfsr(state: u32, yumi: bool) -> u32 {
        if !yumi { return state; }
        let mask = (1 << 31) | (1 << 29) | (1 << 26) | (1 << 25);
        if state & 1 == 1 { (state >> 1) ^ mask } else { state >> 1 }
    }

    // Reset first
    reset_i_drv.write(Logic::One);
    yumi_i_drv.write(Logic::Zero);
    exec.tick_clock(&mut clk);
    test.record_cycle(0,
        &[("reset_i", &[Logic::One]), ("yumi_i", &[Logic::Zero])],
        &[("o", o_obs.read().as_array())],
    );

    // Run N cycles with yumi=1 (advance the LFSR each cycle)
    reset_i_drv.write(Logic::Zero);
    yumi_i_drv.write(Logic::One);

    let num_cycles = 16;
    let mut ref_state = 1u32; // matches Bits::from_u32(1) initial state

    let mut expected_cycles = vec![
        make_cycle(0,
            &[("reset_i", &[Logic::One]), ("yumi_i", &[Logic::Zero])],
            &[("o", Bits::<32>::from_u32(1).as_array())],
        ),
    ];

    for i in 1..=num_cycles {
        ref_state = next_lfsr(ref_state, true);
        exec.tick_clock(&mut clk);

        test.record_cycle(i,
            &[("reset_i", &[Logic::Zero]), ("yumi_i", &[Logic::One])],
            &[("o", o_obs.read().as_array())],
        );
        expected_cycles.push(make_cycle(i,
            &[("reset_i", &[Logic::Zero]), ("yumi_i", &[Logic::One])],
            &[("o", Bits::<32>::from_u32(ref_state).as_array())],
        ));
    }

    let expected = SimulationTrace::from_cycles(expected_cycles);
    test.finish_with_expected(&expected).assert_passed();
}

