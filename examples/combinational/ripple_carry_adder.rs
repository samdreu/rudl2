use copper_core::port::*;
use copper_core::types::*;
use copper_sim::*;
use copper_macros::hardware;

// Combinational logic used inside a module is just a plain Rust function.
fn full_adder(a: Logic, b: Logic, cin: Logic) -> (Logic, Logic) {
    let sum  = a ^ b ^ cin;
    let cout = (a & b) | (cin & (a ^ b));
    (sum, cout)
}

#[hardware(combinational)]
fn ripple_carry_adder<const N: usize>(
    a_i: In<Bits<N>, ()>,
    b_i: In<Bits<N>, ()>,
    sum_o: Out<Bits<N>, ()>,
    cout_o: Out<Logic, ()>,
) {
    let a = a_i.read();
    let b = b_i.read();

    let mut sum_bits = [Logic::Zero; N];
    let mut carry    = Logic::Zero;

    for i in 0..N {
        let (s, c) = full_adder(a[i], b[i], carry);
        sum_bits[i] = s;
        carry = c;
    }

    sum_o.write(Bits::from_array(sum_bits));
    cout_o.write(carry);
}

fn main() {
    const N: usize = 8;

    let mut exec = HardwareExecutor::new();

    let (a_drv, a_in) = wire::<Bits<N>, ()>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<N>, ()>(Bits::zero());
    let (sum_out, sum_obs) = wire::<Bits<N>, ()>(Bits::zero());
    let (cout_out, cout_obs) = wire::<Logic, ()>(Logic::Zero);

    let dhs = vec![sum_out.dirty_handle(), cout_out.dirty_handle()];
    exec.spawn_wired(ripple_carry_adder::<N>(a_in, b_in, sum_out, cout_out), dhs);

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
        a_drv.write(a);
        b_drv.write(b);
        exec.poll_tasks();
        test.record_cycle(
            i,
            &[("a_i", a.as_array()), ("b_i", b.as_array())],
            &[("s_o", sum_obs.read().as_array()), ("c_o", &[cout_obs.read()])],
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
