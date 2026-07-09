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


fn priority_encode<const N: usize, const N_LOG: usize>(
    inputs: Bits<N>,
) -> (Bits<N_LOG>, Logic) {
    const { assert!(N_LOG == safe_clog2(N), "N_LOG must equal safe_clog2(N)") };

    let mut result = Bits::zero();
    let mut valid = Logic::Zero;

    for i in 0..N {
        if inputs[i] == Logic::One {
            result = Bits::from_usize(i);
            valid = Logic::One;
        }
    }

    (result, valid)
}



fn main() {
    const N: usize = 8;
    const N_LOG: usize = safe_clog2(N); // 3

    let mut test = HardwareTest::new("priority_encode")
        .with_verilog("examples/combinational/sv/priority_encode.sv")
        .with_waveform("waveforms/priority_encode.vcd");

    // (inputs, expected_result, expected_valid) — MSB has highest priority
    let cases: &[(Bits<N>, Bits<N_LOG>, Logic)] = &[
        (Bits::from_u8(0b00000000), Bits::from_lit::<0>(), Logic::Zero), // no bits set
        (Bits::from_u8(0b00000001), Bits::from_lit::<0>(), Logic::One),  // only bit 0
        (Bits::from_u8(0b00000010), Bits::from_lit::<1>(), Logic::One),  // only bit 1
        (Bits::from_u8(0b10000000), Bits::from_lit::<7>(), Logic::One),  // only bit 7
        (Bits::from_u8(0b00000011), Bits::from_lit::<1>(), Logic::One),  // bits 0,1  → bit 1 wins
        (Bits::from_u8(0b00110000), Bits::from_lit::<5>(), Logic::One),  // bits 4,5  → bit 5 wins
        (Bits::from_u8(0b10000001), Bits::from_lit::<7>(), Logic::One),  // bits 0,7  → bit 7 wins
        (Bits::from_u8(0b11111111), Bits::from_lit::<7>(), Logic::One),  // all set   → bit 7 wins
    ];

    for (i, &(inputs, _exp_result, _exp_valid)) in cases.iter().enumerate() {
        let (result, valid) = priority_encode::<N, N_LOG>(inputs);
        test.record_cycle(
            i,
            &[("inputs", inputs.as_array())],
            &[("result", result.as_array()), ("valid", &[valid])],
        );
    }

    let expected = SimulationTrace::from_cycles(
        cases.iter().enumerate().map(|(i, &(inputs, exp_result, exp_valid))| {
            make_cycle(
                i,
                &[("inputs", inputs.as_array())],
                &[("result", exp_result.as_array()), ("valid", &[exp_valid])],
            )
        }).collect(),
    );

    test.finish_with_expected(&expected).assert_passed();
}
