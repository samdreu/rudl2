// A register assigned TWICE inside one branch — the mod-N counter idiom.
//
// `t = t + 1; if t == N { t = 0; ... }` is how anyone writes a divider, and it is
// the shape that used to lose its reset.
//
// Both outputs are `RegOut`, and both are written BEFORE the updates, not after.
// Writing after would put the value in the sequential-forwarding family (D1) and
// this file would be measuring that instead. Written first, the value is simply
// the register's current one and the only thing under test is whether the branch
// kept its last assignment.

/// The reset lives inside a `match` arm. This is the shape that silently
/// miscompiled: the arm's second assignment to `t` was dropped, so `t` counted
/// past 3 and wrapped at 16 instead.
#[hardware(sequential)]
async fn divider(clk: Clock<MainClk>, tick_out: RegOut<Bits<8>, MainClk>) {
    let mut st: Bits<2> = Bits::zero();
    let mut t: Bits<4> = Bits::zero();
    let mut n: Bits<8> = Bits::zero();

    loop {
        tick_out.write(n);
        match st {
            _ => {
                t = t + Bits::from_lit::<1>();
                if t == Bits::from_lit::<3>() {
                    t = Bits::zero();
                    n = n + Bits::from_lit::<1>();
                }
                st = Bits::zero();
            }
        }
        clk.tick().await;
    }
}

/// The same rewrite one level down: inside an `if` branch, a nested `if` rewrites
/// the register again. The two assignments must be in the SAME branch — a
/// top-level assignment followed by a branch assignment does not reproduce it,
/// because each branch then holds only one.
#[hardware(sequential)]
async fn wrapping_counter(
    clk: Clock<MainClk>,
    up: In<Logic, MainClk>,
    out: RegOut<Bits<8>, MainClk>,
) {
    let mut v: Bits<8> = Bits::zero();

    loop {
        out.write(v);
        if up.read() == Logic::One {
            v = v + Bits::from_lit::<1>();
            if v == Bits::from_lit::<5>() {
                v = Bits::zero();
            }
        }
        clk.tick().await;
    }
}
