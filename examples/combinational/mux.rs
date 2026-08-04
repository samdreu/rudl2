use copper_core::port::*;
use copper_core::types::*;
use copper_sim::*;
use copper_macros::hardware;

const fn safe_clog2(n: usize) -> usize {
    match n {
        0 | 1 => 0,
        _ => {
            let mut bits = 0;
            let mut v = n - 1;
            while v > 0 { 
                v >>=1; bits += 1;
            }
            bits
        }
    }
}

#[hardware(combinational)]
fn mux<
    const WIDTH_P: usize,
    const ELS_P: usize,
    const LG_ELS_LP: usize,
> (
    data_i: In<[Bits<WIDTH_P>; ELS_P], ()>,
    sel_i: In<Bits<LG_ELS_LP>, ()>,
    data_o: Out<Bits<WIDTH_P>, ()>,
) {
    const { assert!(LG_ELS_LP == safe_clog2(ELS_P), "LG_ELS_LP must equal safe_clog2(ELS_P)") };

    if ELS_P == 1 {
        data_o.write(data_i.read()[0]);
    } else {
        data_o.write(data_i.read()[sel_i.read().as_u128() as usize]);
    }
}

fn main() {
    const WIDTH: usize = 8;
    const ELS: usize = 4;
    const LG_ELS: usize = safe_clog2(ELS); // 2

    let mut executor = HardwareExecutor::new();

    let (data_drv, data_in)  = wire::<[Bits<WIDTH>; ELS], ()>([Bits::zero(); ELS]);
    let (sel_drv,  sel_in)   = wire::<Bits<LG_ELS>, ()>(Bits::zero());
    let (data_out, data_obs) = wire::<Bits<WIDTH>, ()>(Bits::zero());

    let dh = data_out.dirty_handle();

    let reads = vec![data_in.wire_id(), sel_in.wire_id()];
    executor.spawn_wired(
        mux::<WIDTH, ELS, LG_ELS>(data_in, sel_in, data_out),
        vec![dh],
        reads,
    );

    let mut test = HardwareTest::new("bsg_mux")
        .with_verilog("examples/combinational/sv/mux.sv")
        .with_waveform("waveforms/bsg_mux.vcd");

    // Fixed input data: distinct values so any selection error is visible.
    let data: [Bits<WIDTH>; ELS] = [
        Bits::from_u8(0xAA),
        Bits::from_u8(0xBB),
        Bits::from_u8(0xCC),
        Bits::from_u8(0xDD),
    ];

    // (sel, expected_output): one case per input, cycling through all sel values.
    let cases: &[(Bits<LG_ELS>, Bits<WIDTH>)] = &[
        (Bits::from_slice(&[Logic::Zero, Logic::Zero]), Bits::from_u8(0xAA)), // sel=0 → data[0]
        (Bits::from_slice(&[Logic::One,  Logic::Zero]), Bits::from_u8(0xBB)), // sel=1 → data[1]
        (Bits::from_slice(&[Logic::Zero, Logic::One ]), Bits::from_u8(0xCC)), // sel=2 → data[2]
        (Bits::from_slice(&[Logic::One,  Logic::One ]), Bits::from_u8(0xDD)), // sel=3 → data[3]
    ];

    // Flatten [Bits<WIDTH>; ELS] → Vec<Logic> (element 0 at LSBs) for the Verilator port.
    let data_flat: Vec<Logic> = data.iter()
        .flat_map(|b| b.as_array().iter().copied())
        .collect();

    for (i, (sel, _expected)) in cases.iter().enumerate() {
        data_drv.write(data);
        sel_drv.write(*sel);
        executor.poll_tasks();

        let output = data_obs.read();
        test.record_cycle(
            i,
            &[("data_i", &data_flat), ("sel_i", sel.as_array())],
            &[("data_o", output.as_array())],
        );
    }

    let expected = SimulationTrace::from_cycles(
        cases.iter().enumerate().map(|(i, (sel, expected_out))| {
            make_cycle(
                i,
                &[("data_i", &data_flat), ("sel_i", sel.as_array())],
                &[("data_o", expected_out.as_array())],
            )
        }).collect(),
    );

    test.finish_with_expected(&expected).assert_passed();
}
