// Single source of truth for the branch-local-temporary equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
//
// The shape under test is `up` and `dn`: `let` bindings declared INSIDE the
// else-branch with COMPUTED initializers, read only within that same branch.
// Rust scopes them to the branch, so nothing outside can observe them and they
// cannot latch — but the transpiler used to report
// "would infer a latch: dn, up assigned on some control paths but not all",
// because the branch-local default hoist only handled literal initializers.
//
// `count` is the control in the same module: a register driven conditionally,
// which is the implicit-HOLD idiom (bsg_dff_en), not a latch. It must keep its
// hold and must never be given an unconditional default.
#[hardware(sequential)]
pub async fn branch_local_counter(
    clk: Clock<MainClk>,
    rst_i: In<Logic, MainClk>,
    up_i: In<Logic, MainClk>,
    dn_i: In<Logic, MainClk>,
    count_o: Out<Bits<4>, MainClk>,
) {
    let mut count = Bits::<4>::zero();
    loop {
        clk.tick().await;
        if rst_i.read() == Logic::One {
            count = Bits::zero();
        } else {
            let up = up_i.read();
            let dn = dn_i.read();
            if up == Logic::One {
                count = count + Bits::from_lit::<1>();
            }
            if dn == Logic::One {
                count = count - Bits::from_lit::<1>();
            }
        }
        count_o.write(count);
    }
}
