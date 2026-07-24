// Single source of truth for the priority_encode equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// A const-generic MSB-priority encoder (highest set bit wins) — exercises
// symbolic width, a `for` loop, `Bits::from_usize`, and two outputs.
// Verilated at N=8, N_LOG=3.
const fn safe_clog2(n: usize) -> usize {
    match n {
        0 | 1 => 0,
        _ => {
            let mut bits = 0;
            let mut v = n - 1;
            while v > 0 { v >>= 1; bits += 1; }
            bits
        }
    }
}

#[hardware(combinational)]
fn priority_encode<const N: usize, const N_LOG: usize>(
    inputs: In<Bits<N>, ()>,
    result: Out<Bits<N_LOG>, ()>,
    valid: Out<Logic, ()>,
) {
    const { assert!(N_LOG == safe_clog2(N), "N_LOG must equal safe_clog2(N)") };

    let in_val = inputs.read();
    let mut res = Bits::zero();
    let mut v = Logic::Zero;

    for i in 0..N {
        if in_val[i] == Logic::One {
            res = Bits::from_usize(i);
            v = Logic::One;
        }
    }

    result.write(res);
    valid.write(v);
}
