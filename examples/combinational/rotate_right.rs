use copper_core::types::*;
use copper_sim::*;

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

fn rotate_right<const N: usize, const N_LOG: usize>(
    data_i: Bits<N>,
    rot_i: Bits<N_LOG>,
) -> Bits<N> {
    const { assert!(N_LOG == safe_clog2(N), "N_LOG must equal safe_clog2(N)") };

    let mut o = Bits::zero();
    let rot = rot_i.as_u128() as usize;

    for i in 0..N {
        o[i] = data_i[(i + rot) % N];
    }

    o
}

fn main() {
    const N: usize = 8;
    const N_LOG: usize = safe_clog2(N); // 3

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
        let result = rotate_right::<N, N_LOG>(data, rot);
        test.record_cycle(
            i,
            &[("data_i", data.as_array()), ("rot_i", rot.as_array())],
            &[("o", result.as_array())],
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
