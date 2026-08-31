// Single source of truth for the ripple_carry_adder equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// A const-generic N-bit adder with an inline full-adder and a scalar carry
// threaded across iterations — exercises a `for` loop over symbolic width, LHS
// bit-assign into a `Bits<N>`, a `Logic` accumulator reassigned each iteration,
// and two outputs (a `Bits<N>` sum and a `Logic` carry-out). Verilated at N=8.
#[hardware(combinational)]
pub fn ripple_carry_adder<const N: usize>(
    a_i: In<Bits<N>, ()>,
    b_i: In<Bits<N>, ()>,
    sum_o: Out<Bits<N>, ()>,
    cout_o: Out<Logic, ()>,
) {
    let a = a_i.read();
    let b = b_i.read();

    let mut sum = Bits::zero();
    let mut carry = Logic::Zero;

    for i in 0..N {
        sum[i] = a[i] ^ b[i] ^ carry;
        carry = (a[i] & b[i]) | (carry & (a[i] ^ b[i]));
    }

    sum_o.write(sum);
    cout_o.write(carry);
}
