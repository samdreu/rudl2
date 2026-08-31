// Single source of truth for the wide-datapath equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// A 16-bit multi-output combinational datapath: three results driven from the same
// two inputs — exercises a WIDE datapath (Bits<16>, past the 8-bit boundary), the
// `*` multiply operator, `-` (two's-complement subtract), and output fan-in (three
// `Out`s from shared reads).
#[hardware(combinational)]
fn datapath16(
    a: In<Bits<16>, ()>,
    b: In<Bits<16>, ()>,
    sum: Out<Bits<16>, ()>,
    prod: Out<Bits<16>, ()>,
    diff: Out<Bits<16>, ()>,
) {
    let av = a.read();
    let bv = b.read();
    sum.write(av + bv);
    prod.write(av * bv);
    diff.write(av - bv);
}
