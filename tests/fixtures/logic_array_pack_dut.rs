// Single source of truth for the `[Logic; N]` array-local equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
//
// Assembles a 16-bit word from four 4-bit inputs through a raw `[Logic; 16]`
// array local — nibble `n` lands at bits `4n..4n+3` — then packs it with
// `Bits::from_slice`. This is the natural idiom `examples/basejump/sipo_block`
// uses, and the one that used to fail transpilation with "cannot infer bit
// width" (see `copper-codegen/tests/transpile_inference_gaps.rs`).
//
// What it exercises that a `Bits` local does not: an array-repeat initialiser
// (`[Logic::Zero; 16]`), indexed writes with computed indices into that array,
// and the identity pack back to `Bits<16>`.
#[hardware(combinational)]
pub fn logic_array_pack(
    n0_i: In<Bits<4>, ()>,
    n1_i: In<Bits<4>, ()>,
    n2_i: In<Bits<4>, ()>,
    n3_i: In<Bits<4>, ()>,
    word_o: Out<Bits<16>, ()>,
) {
    let n0 = n0_i.read();
    let n1 = n1_i.read();
    let n2 = n2_i.read();
    let n3 = n3_i.read();

    let mut bits = [Logic::Zero; 16];
    for k in 0..4 {
        bits[k] = n0[k];
        bits[4 + k] = n1[k];
        bits[8 + k] = n2[k];
        bits[12 + k] = n3[k];
    }
    word_o.write(Bits::from_slice(&bits));
}
