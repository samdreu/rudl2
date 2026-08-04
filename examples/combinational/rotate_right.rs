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
                v >>= 1; bits += 1;
            }
            bits
        }
    }
}

#[hardware(combinational)]
fn rotate_right<const N: usize, const N_LOG: usize>(
    data_i: In<Bits<N>, ()>,
    rot_i: In<Bits<N_LOG>, ()>,
    o: Out<Bits<N>, ()>,
) {
    const { assert!(N_LOG == safe_clog2(N), "N_LOG must equal safe_clog2(N)") };

    let data = data_i.read();
    let rot = rot_i.read().as_u128() as usize;

    let mut o_val = Bits::zero();
    for i in 0..N {
        o_val[i] = data[(i + rot) % N];
    }

    o.write(o_val);
}

fn main() {
    const N: usize = 8;
    const N_LOG: usize = safe_clog2(N); // 3

    let mut exec = HardwareExecutor::new();

    let (data_drv, data_in) = wire::<Bits<N>, ()>(Bits::zero());
    let (rot_drv, rot_in) = wire::<Bits<N_LOG>, ()>(Bits::zero());
    let (o_out, o_obs) = wire::<Bits<N>, ()>(Bits::zero());

    let dh = o_out.dirty_handle();
    let reads = vec![data_in.wire_id(), rot_in.wire_id()];
    exec.spawn_wired(rotate_right::<N, N_LOG>(data_in, rot_in, o_out), vec![dh], reads);

    let mut test = HardwareTest::new("bsg_rotate_right")
        .with_verilog("examples/combinational/sv/rotate_right.sv")
        .with_waveform("waveforms/rotate_right.vcd");

    // (data, rot, expected)
    let cases: &[(Bits<N>, Bits<N_LOG>, Bits<N>)] = &[
        (Bits::from_u8(0b10110001), Bits::from_lit::<1>(), Bits::from_u8(0b11011000)), // rotate by 1
        (Bits::from_u8(0b10110001), Bits::from_lit::<3>(), Bits::from_u8(0b00110110)), // rotate by 3
        (Bits::from_u8(0b10110001), Bits::from_lit::<4>(), Bits::from_u8(0b00011011)), // rotate by 4
        (Bits::from_u8(0b11111111), Bits::from_lit::<5>(), Bits::from_u8(0b11111111)), // all ones, any rot
        (Bits::from_u8(0b00000001), Bits::from_lit::<1>(), Bits::from_u8(0b10000000)), // single bit
        (Bits::from_u8(0b10000000), Bits::from_lit::<7>(), Bits::from_u8(0b00000001)), // MSB wraps to LSB
        (Bits::from_u8(0b10110001), Bits::from_lit::<0>(), Bits::from_u8(0b10110001)), // rotate by 0
    ];

    for (i, &(data, rot, _expected)) in cases.iter().enumerate() {
        data_drv.write(data);
        rot_drv.write(rot);
        exec.poll_tasks();
        test.record_cycle(
            i,
            &[("data_i", data.as_array()), ("rot_i", rot.as_array())],
            &[("o", o_obs.read().as_array())],
        );
    }

    let expected = SimulationTrace::from_cycles(
        cases.iter().enumerate().map(|(i, &(data, rot, exp))| {
            make_cycle(
                i,
                &[("data_i", data.as_array()), ("rot_i", rot.as_array())],
                &[("o", exp.as_array())],
            )
        }).collect(),
    );

    test.finish_with_expected(&expected).assert_passed();
}
