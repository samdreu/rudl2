// Single source of truth for the modulo-datapath equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// Exercises the `%` (remainder) operator, which the transpiler supports (unlike
// `/`, which it rejects — see copper-codegen/tests/unsupported_constructs.rs). The
// divisor is kept non-zero by the stimulus (a zero divisor is X in hardware).
#[hardware(combinational)]
fn mod8(a: In<Bits<8>, ()>, b: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(a.read() % b.read());
}
