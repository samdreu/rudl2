// Single source of truth for the one_bit_comparator equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation, so the
// simulated and transpiled designs are byte-identical by construction.
// A purely combinational XNOR built from products — exercises `Logic` ports,
// unary `!`, and bitwise `&`/`|` in a clockless (`()` domain) module.
#[hardware(combinational)]
pub fn one_bit_comparator(i0: In<Logic, ()>, i1: In<Logic, ()>, eq: Out<Logic, ()>) {
    let p0 = !i0.read() & !i1.read();
    let p1 = i0.read() & i1.read();
    eq.write(p0 | p1);
}
