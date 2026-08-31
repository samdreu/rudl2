// Single source of truth for the file-scope-const equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
//
// The point of the fixture is the two `const` items: `WIDTH` appears in a PORT
// WIDTH and again as a loop bound, `ADDR_W` in another port width and in a local
// type. Neither is reachable from the `ItemFn`, so before file-scope consts
// lowered to `localparam`s this module reported `undefined variable 'WIDTH'` and
// had no transpiled path at all.
//
// The design itself is a one-hot encoder (BaseJump `bsg_encode_one_hot`'s
// behaviour): addr_o = index of the set bit, v_o = (input != 0).
const WIDTH: usize = 8;
const ADDR_W: usize = 3;

#[hardware(combinational)]
pub fn const_encoder(
    i: In<Bits<WIDTH>, ()>,
    addr_o: Out<Bits<ADDR_W>, ()>,
    v_o: Out<Logic, ()>,
) {
    let inp = i.read();
    let mut addr = Bits::<ADDR_W>::zero();
    let mut valid = Logic::Zero;
    for k in 0..WIDTH {
        if inp[k] == Logic::One {
            addr = Bits::from_usize(k);
            valid = Logic::One;
        }
    }
    addr_o.write(addr);
    v_o.write(valid);
}
