use copper_core::types::*;
use copper_sim::*;

fn full_adder(a: Logic, b: Logic, cin: Logic) -> (Logic, Logic) {
    let sum  = a ^ b ^ cin;
    let cout = (a & b) | (cin & (a ^ b));
    (sum, cout)
}

fn ripple_carry_adder<const N: usize>(a_i: Bits<N>, b_i: Bits<N>) -> (Bits<N>, Logic) {
    let mut sum_bits = [Logic::Zero; N];
    let mut carry    = Logic::Zero;

    for i in 0..N {
        let (s, c) = full_adder(a_i[i], b_i[i], carry);
        sum_bits[i] = s;
        carry = c;
    }

    (Bits::from_array(sum_bits), carry)
}


fn main() {
    const N: usize = 8;

    let mut test = HardwareTest::new("bsg_adder_ripple_carry")
        .with_verilog("examples/combinational/sv/ripple_carry_adder.sv")
        .with_waveform("waveforms/ripple_carry_adder.vcd");

    // (a, b, expected_sum, expected_carry)
    let cases: &[(Bits<N>, Bits<N>, Bits<N>, Logic)] = &[
        (Bits::from_u8(0),   Bits::from_u8(0),   Bits::from_u8(0),   Logic::Zero), // 0 + 0
        (Bits::from_u8(1),   Bits::from_u8(2),   Bits::from_u8(3),   Logic::Zero), // basic sum
        (Bits::from_u8(127), Bits::from_u8(1),   Bits::from_u8(128), Logic::Zero), // no carry
        (Bits::from_u8(255), Bits::from_u8(0),   Bits::from_u8(255), Logic::Zero), // max + 0
        (Bits::from_u8(255), Bits::from_u8(1),   Bits::from_u8(0),   Logic::One),  // overflow
        (Bits::from_u8(128), Bits::from_u8(128), Bits::from_u8(0),   Logic::One),  // half + half
        (Bits::from_u8(200), Bits::from_u8(100), Bits::from_u8(44),  Logic::One),  // mid overflow
        (Bits::from_u8(255), Bits::from_u8(255), Bits::from_u8(254), Logic::One),  // max + max
        (Bits::from_u8(15),  Bits::from_u8(240), Bits::from_u8(255), Logic::Zero), // complement
    ];

    for (i, &(a, b, _expected_sum, _expected_carry)) in cases.iter().enumerate() {
        let (sum, carry) = ripple_carry_adder::<N>(a, b);
        test.record_cycle(
            i,
            &[("a_i", a.as_array()), ("b_i", b.as_array())],
            &[("s_o", sum.as_array()), ("c_o", &[carry])],
        );
    }

    let expected = SimulationTrace::from_cycles(
        cases.iter().enumerate().map(|(i, &(a, b, expected_sum, expected_carry))| {
            make_cycle(
                i,
                &[("a_i", a.as_array()), ("b_i", b.as_array())],
                &[("s_o", expected_sum.as_array()), ("c_o", &[expected_carry])],
            )
        }).collect(),
    );

    test.finish_with_expected(&expected).assert_passed();
}
