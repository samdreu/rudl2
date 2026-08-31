// Single source of truth for the rotate_right equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// A const-generic barrel rotate-right — exercises symbolic width, a `for` loop,
// LHS bit-assign, dynamic bit-select, and `%`. Verilated at N=8, N_LOG=3.
const fn safe_clog2(n: usize) -> usize {
    match n {
        0 | 1 => 0,
        _ => {
            let mut bits = 0;
            let mut v = n - 1;
            while v > 0 {
                v >>= 1;
                bits += 1;
            }
            bits
        }
    }
}

#[hardware(combinational)]
pub fn rotate_right<const N: usize, const N_LOG: usize>(
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
