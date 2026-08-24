//! Branch-local temporaries and the latch check.
//!
//! A `let` inside a branch is scoped to that branch in Rust, so nothing outside
//! can observe it and it cannot latch — but SystemVerilog needs an unconditional
//! default for the synthesizer to agree. `hoist_branch_local_defaults` supplies
//! one; these tests pin *which* names get one, because the rule is only safe
//! because of what it leaves alone.
//!
//! The distinction that matters: a conditionally-driven **register** looks
//! identical to a latch by the "not assigned on all paths" test, but it is the
//! implicit-HOLD idiom (verified against BaseJump's `bsg_dff_en`). Defaulting it
//! would clear it on every untaken path — a silent, cycle-accurate wrong answer
//! rather than a compile error. `tests/branch_local_temp_equivalence.rs` at the
//! repo root carries the behavioural half under Verilator.

fn transpile(src: &str) -> Result<String, String> {
    copper_codegen::transpile_source(src, None, &copper_codegen::EmitConfig::default())
        .map_err(|e| e.to_string())
}

/// A computed initializer cannot be hoisted the way a literal one can — moving
/// it would evaluate it on paths the source never runs it on — so the default
/// goes to the top and the computation stays in the branch.
#[test]
fn a_branch_local_with_a_computed_initializer_gets_a_default_and_keeps_its_computation() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, r: In<Logic, MainClk>, up_i: In<Logic, MainClk>, o: Out<Bits<3>, MainClk>) {
    let mut count = Bits::<3>::zero();
    loop {
        clk.tick().await;
        if r.read() == Logic::One {
            count = Bits::zero();
        } else {
            let up = up_i.read();
            if up == Logic::One { count = count + Bits::from_lit::<1>(); }
        }
        o.write(count);
    }
}
"#;
    let sv = transpile(src).expect("a branch-local temporary must not read as a latch");
    assert!(
        sv.contains("up = 1'b0;"),
        "the temporary needs an unconditional default, got:\n{sv}"
    );
    assert!(
        sv.contains("up = up_i;"),
        "the computation must stay in the branch, got:\n{sv}"
    );
    // The default precedes the branch that overwrites it.
    let default_at = sv.find("up = 1'b0;").expect("default present");
    let compute_at = sv.find("up = up_i;").expect("computation present");
    assert!(default_at < compute_at, "the default must come first, got:\n{sv}");
}

/// The invariant this rule depends on. A register driven on only some paths is a
/// HOLD, not a latch: `count` keeps its value when neither branch writes it, and
/// an unconditional `count = 0` in the combinational block would destroy that.
#[test]
fn a_conditionally_driven_register_is_never_given_a_default() {
    let src = r#"
#[hardware(sequential)]
async fn m(clk: Clock<MainClk>, en_i: In<Logic, MainClk>, d_i: In<Bits<4>, MainClk>, o: Out<Bits<4>, MainClk>) {
    let mut q = Bits::<4>::zero();
    loop {
        clk.tick().await;
        if en_i.read() == Logic::One {
            let d = d_i.read();
            q = d;
        }
        o.write(q);
    }
}
"#;
    let sv = transpile(src).expect("the enabled-register idiom must transpile");
    // The branch-local `d` is defaulted...
    assert!(sv.contains("d = 4'd0;") || sv.contains("d = '0;"), "got:\n{sv}");
    // ...but the register holds, driven from always_ff with an explicit
    // else-value of itself. A `q = ...` default in always_comb would mean the
    // register had been treated as a branch-local.
    assert!(
        sv.contains("always_ff"),
        "the register must be driven from always_ff, got:\n{sv}"
    );
    assert!(
        sv.contains("q <= "),
        "the register must keep a non-blocking update, got:\n{sv}"
    );
    for line in sv.lines() {
        let t = line.trim();
        assert!(
            !(t.starts_with("q = ")),
            "a register must never get a blocking default, got:\n{sv}"
        );
    }
}

/// The pre-existing literal path is unchanged: a constant initializer is moved
/// to the top whole, rather than being split into a default plus a redundant
/// re-assignment.
#[test]
fn a_branch_local_with_a_literal_initializer_still_moves_whole() {
    let src = r#"
#[hardware(combinational)]
fn m(sel: In<Logic, ()>, o: Out<Bits<4>, ()>) {
    let mut r = Bits::<4>::zero();
    if sel.read() == Logic::One {
        let bump = Bits::<4>::from_lit::<3>();
        r = bump;
    }
    o.write(r);
}
"#;
    let sv = transpile(src).expect("transpiles");
    let assigns = sv.matches("bump = ").count();
    assert_eq!(
        assigns, 1,
        "a literal initializer is its own default and must not be duplicated, got:\n{sv}"
    );
}
