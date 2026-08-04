// Single source of truth for the gray_to_binary equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// A concrete (width=8) Gray→binary decoder via a descending prefix-XOR — exercises
// a `for` loop with inline descending index arithmetic and a loop-carried
// bit-read (`b[k+1]` was written a prior iteration). Modeled on BaseJump STL
// `bsg_gray_to_binary`.
#[hardware(combinational)]
pub fn gray_to_binary(gray_i: In<Bits<8>, ()>, binary_o: Out<Bits<8>, ()>) {
    let g = gray_i.read();
    let mut b = Bits::zero();
    b[7] = g[7];
    for i in 1..8 {
        b[7 - i] = g[7 - i] ^ b[7 - i + 1];
    }
    binary_o.write(b);
}
