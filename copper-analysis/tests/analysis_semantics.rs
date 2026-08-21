//! End-to-end coverage of the shared compile-time analysis (`copper-analysis`)
//! through its **public API**, on hand-written `#[hardware]` snippets whose
//! expected facts are known by construction.
//!
//! The unit tests in `src/cfg.rs` exercise the CFG builder internals; this file
//! pins the *published* contract — [`infer_registers`], [`check_reachability`],
//! [`check_definite_assignment`], [`classify_reads`], [`reference_sv_registers`],
//! and the [`RegMatch`] asserts — the surface both front-ends (macro + transpiler)
//! actually consume. A regression here is a regression in a compile-time guarantee.

use copper_analysis::{
    check_definite_assignment, check_reachability, classify_reads, infer_registers,
    multi_write_collapse, reference_sv_registers, Cfg, ReadTiming, RegMatch,
};
use syn::ItemFn;

/// Parse a single `#[hardware]` fn from source, as both front-ends do.
fn f(src: &str) -> ItemFn {
    syn::parse_str(src).expect("snippet parses as a single hardware fn")
}

/// Sorted register-name set inferred from `src`.
fn regs(src: &str) -> Vec<String> {
    infer_registers(&f(src))
}

// ── register inference (T1: backward liveness over the CFG) ──────────────────

/// A one-register counter: `count` is defined-in-loop and live across the tick.
#[test]
fn counter_infers_its_accumulator() {
    let src = r#"
        #[hardware(sequential)]
        async fn counter(clk: Clock<C>, step: In<Bits<8>, C>, out: Out<Bits<8>, C>) {
            let mut count: Bits<8> = Bits::zero();
            loop {
                out.write(count);
                clk.tick().await;
                count = count + step.read();
            }
        }
    "#;
    assert_eq!(regs(src), vec!["count"]);
}

/// A pre-loop binding that is only *read* in the loop is a constant/wire, not a
/// flip-flop — it fails criterion (a) "defined inside the loop". (The `lfsr`
/// `xor_mask` case.) Ports never count as registers.
#[test]
fn read_only_preloop_binding_is_not_a_register() {
    let src = r#"
        #[hardware(sequential)]
        async fn lfsr(clk: Clock<C>, out: Out<Bits<8>, C>) {
            let xor_mask: Bits<8> = Bits::from_lit::<0xB4>();
            let mut state: Bits<8> = Bits::from_lit::<1>();
            loop {
                out.write(state);
                clk.tick().await;
                let lsb = state.get(0);
                state = state.shift_right(1);
                if lsb == Logic::One { state = state ^ xor_mask; }
            }
        }
    "#;
    let r = regs(src);
    assert!(r.contains(&"state".to_string()), "state is the flip-flop: {r:?}");
    assert!(!r.contains(&"xor_mask".to_string()), "xor_mask is a constant, not a register: {r:?}");
}

/// A multi-stage pipeline registers *every* value carried across a tick — the
/// generalization beyond the minimal "pre-loop binding reassigned in loop" slice.
#[test]
fn pipeline_infers_every_stage_register() {
    let src = r#"
        #[hardware(sequential)]
        async fn mac_pipeline(clk: Clock<C>, a: In<Bits<8>, C>, b: In<Bits<8>, C>, c: In<Bits<8>, C>, out: Out<Bits<16>, C>) {
            loop {
                let product = a.read() * b.read();
                let c_s = c.read();
                clk.tick().await;
                let sum = product + c_s;
                clk.tick().await;
                out.write(sum);
                clk.tick().await;
            }
        }
    "#;
    let r = regs(src);
    // `product` and `c_s` cross the first tick; `sum` crosses the second.
    for name in ["product", "c_s", "sum"] {
        assert!(r.contains(&name.to_string()), "expected {name} in inferred registers {r:?}");
    }
}

/// An enum FSM state variable reassigned across the tick is a register.
#[test]
fn fsm_state_variable_is_a_register() {
    let src = r#"
        #[hardware(sequential)]
        async fn det(clk: Clock<C>, in_i: In<Logic, C>, out_o: Out<Logic, C>) {
            let mut state = State::A;
            loop {
                if matches!(state, State::D) { out_o.write(Logic::One); } else { out_o.write(Logic::Zero); }
                clk.tick().await;
                state = match (state, in_i.read()) { _ => State::A };
            }
        }
    "#;
    assert_eq!(regs(src), vec!["state"]);
}

/// A combinational module has no clocked loop, so no registers and `Cfg::build`
/// declines to build a sequential graph.
#[test]
fn combinational_module_has_no_registers() {
    let src = r#"
        #[hardware(combinational)]
        fn andgate(a: In<Logic, ()>, b: In<Logic, ()>, o: Out<Logic, ()>) {
            o.write(a.read() & b.read());
        }
    "#;
    assert!(regs(src).is_empty(), "combinational module infers no registers");
    assert!(Cfg::build(&f(src)).is_none(), "no sequential CFG for a clockless body");
}

// ── reachability well-formedness (every loop path must reach a tick) ─────────

#[test]
fn well_formed_sequential_loop_passes_reachability() {
    let src = r#"
        #[hardware(sequential)]
        async fn ok(clk: Clock<C>, out: Out<Bits<8>, C>) {
            let mut n: Bits<8> = Bits::zero();
            loop {
                out.write(n);
                clk.tick().await;
                n = n + Bits::from_lit::<1>();
            }
        }
    "#;
    assert!(check_reachability(&f(src)).is_ok());
}

/// A loop body with no tick at all is a zero-time combinational cycle — rejected.
#[test]
fn tickless_loop_is_rejected() {
    let src = r#"
        #[hardware(sequential)]
        async fn bad(clk: Clock<C>, out: Out<Bits<8>, C>) {
            let mut n: Bits<8> = Bits::zero();
            loop {
                out.write(n);
                n = n + Bits::from_lit::<1>();
            }
        }
    "#;
    let err = check_reachability(&f(src)).expect_err("a tickless loop must be rejected");
    assert!(
        err.to_string().contains("clk.tick().await"),
        "error should name the missing tick: {err}"
    );
}

/// A branch that loops back to the top without ticking (the `else` path never
/// ticks) is also a tickless cycle — rejected even though a *sibling* path ticks.
#[test]
fn partial_tickless_path_is_rejected() {
    let src = r#"
        #[hardware(sequential)]
        async fn bad(clk: Clock<C>, sel: In<Logic, C>, out: Out<Bits<8>, C>) {
            let mut n: Bits<8> = Bits::zero();
            loop {
                if sel.read() == Logic::One {
                    clk.tick().await;
                }
                out.write(n);
                n = n + Bits::from_lit::<1>();
            }
        }
    "#;
    // The `sel == 0` path returns to the loop head without awaiting a tick.
    assert!(check_reachability(&f(src)).is_err(), "the un-ticked branch is a zero-time cycle");
}

/// Uneven per-branch tick *counts* are legal so long as every path ticks at least
/// once — the plan explicitly permits this and reachability must not reject it.
#[test]
fn uneven_but_nonzero_tick_counts_are_well_formed() {
    let src = r#"
        #[hardware(sequential)]
        async fn ok(clk: Clock<C>, sel: In<Logic, C>, out: Out<Bits<8>, C>) {
            let mut n: Bits<8> = Bits::zero();
            loop {
                if sel.read() == Logic::One {
                    clk.tick().await;
                    clk.tick().await;
                } else {
                    clk.tick().await;
                }
                out.write(n);
                n = n + Bits::from_lit::<1>();
            }
        }
    "#;
    assert!(check_reachability(&f(src)).is_ok());
}

// ── definite assignment (combinational outputs: all paths or none) ───────────

#[test]
fn fully_driven_combinational_output_is_ok() {
    let src = r#"
        #[hardware(combinational)]
        fn mux(sel: In<Logic, ()>, a: In<Bits<8>, ()>, b: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
            if sel.read() == Logic::One { o.write(a.read()); } else { o.write(b.read()); }
        }
    "#;
    assert!(check_definite_assignment(&f(src)).is_ok());
}

/// An output written on the `then` path but not the (implicit) `else` path infers
/// a latch — rejected, naming the offending output port.
#[test]
fn conditionally_driven_combinational_output_infers_latch() {
    let src = r#"
        #[hardware(combinational)]
        fn leaky(sel: In<Logic, ()>, a: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
            if sel.read() == Logic::One { o.write(a.read()); }
        }
    "#;
    let err = check_definite_assignment(&f(src)).expect_err("partial drive must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("latch"), "error should mention a latch: {msg}");
    assert!(msg.contains('o'), "error should name output `o`: {msg}");
}

/// A sequential `Out` legitimately *holds* when unwritten (the enabled-register
/// idiom, verified sim≡BaseJump on `bsg_dff_en`), so definite-assignment must NOT
/// be imposed on a clocked loop — the public router skips it.
#[test]
fn sequential_partial_output_is_not_a_latch() {
    let src = r#"
        #[hardware(sequential)]
        async fn dff_en(clk: Clock<C>, en: In<Logic, C>, d: In<Bits<8>, C>, q: Out<Bits<8>, C>) {
            loop {
                clk.tick().await;
                if en.read() == Logic::One { q.write(d.read()); }
            }
        }
    "#;
    assert!(check_definite_assignment(&f(src)).is_ok(), "enabled register is not a latch");
}

// ── read-timing classification (item 3) ──────────────────────────────────────

/// Loop-top reads that precede a tick register at the edge → `Deferred`.
#[test]
fn pretick_reads_are_deferred() {
    let src = r#"
        #[hardware(sequential)]
        async fn m(clk: Clock<C>, a: In<Bits<8>, C>, b: In<Bits<8>, C>, out: Out<Bits<8>, C>) {
            loop {
                let s = a.read() + b.read();
                clk.tick().await;
                out.write(s);
            }
        }
    "#;
    assert_eq!(classify_reads(&f(src)), vec![ReadTiming::Deferred, ReadTiming::Deferred]);
}

/// A trailing read after the last tick with nothing following consumes the value
/// the edge produced → `Immediate` (the enabled-register idiom).
#[test]
fn trailing_read_is_immediate() {
    let src = r#"
        #[hardware(sequential)]
        async fn dff_en(clk: Clock<C>, en: In<Logic, C>, q: Out<Logic, C>) {
            loop {
                clk.tick().await;
                if en.read() == Logic::One { q.write(Logic::One); }
            }
        }
    "#;
    assert_eq!(classify_reads(&f(src)), vec![ReadTiming::Immediate]);
}

// ── reference-SV register extraction + G2 structural match ───────────────────

/// `reference_sv_registers` returns nonblocking-assignment targets minus output
/// ports: the genuine internal flip-flops of a hand-written reference.
#[test]
fn reference_sv_extracts_internal_flops_only() {
    let sv = r#"
        module counter(input clk, output [7:0] q);
            reg [7:0] count;
            reg [7:0] q_r;
            always @(posedge clk) begin
                count <= count + 1;
                q_r <= count;
            end
            assign q = q_r;
        endmodule
    "#;
    let regs = reference_sv_registers(sv);
    assert!(regs.contains("count"), "internal reg `count`: {regs:?}");
    assert!(regs.contains("q_r"), "internal reg `q_r`: {regs:?}");
}

/// An `output reg` (Copper's `RegOut` axis) is a port, not internal state, so it is
/// excluded from the reference register set.
#[test]
fn reference_sv_excludes_output_reg_ports() {
    let sv = r#"
        module m(input clk, output reg [7:0] y);
            reg [7:0] acc;
            always @(posedge clk) begin
                acc <= acc + 1;
                y   <= acc;
            end
        endmodule
    "#;
    let regs = reference_sv_registers(sv);
    assert!(regs.contains("acc"), "internal reg retained: {regs:?}");
    assert!(!regs.contains("y"), "output-port reg excluded: {regs:?}");
}

/// The `NameExact` G2 assert passes for a faithful reference that mirrors the
/// design's own names.
#[test]
fn name_exact_match_against_faithful_reference() {
    let dut = r#"
        #[hardware(sequential)]
        async fn counter(clk: Clock<C>, out: Out<Bits<8>, C>) {
            let mut count: Bits<8> = Bits::zero();
            loop {
                out.write(count);
                clk.tick().await;
                count = count + Bits::from_lit::<1>();
            }
        }
    "#;
    let sv = r#"
        module counter(input clk, output [7:0] out);
            reg [7:0] count;
            always @(posedge clk) count <= count + 1;
            assign out = count;
        endmodule
    "#;
    copper_analysis::assert_registers_match_reference_sv(dut, sv, RegMatch::NameExact);
}

/// `StorageEquivalent` matches on flip-flop *count*, tolerating an independent
/// author's different register naming.
#[test]
fn storage_equivalent_match_ignores_names() {
    let dut = r#"
        #[hardware(sequential)]
        async fn counter(clk: Clock<C>, out: Out<Bits<8>, C>) {
            let mut count: Bits<8> = Bits::zero();
            loop {
                out.write(count);
                clk.tick().await;
                count = count + Bits::from_lit::<1>();
            }
        }
    "#;
    // Independent reference: one internal flip-flop, named differently.
    let sv = r#"
        module counter(input clk, output [7:0] out);
            reg [7:0] value_q;
            always @(posedge clk) value_q <= value_q + 1;
            assign out = value_q;
        endmodule
    "#;
    copper_analysis::assert_registers_match_reference_sv(dut, sv, RegMatch::StorageEquivalent);
}

// ── FSM report (small human-readable summary over the CFG) ────────────────────

#[test]
fn fsm_report_counts_tick_boundaries() {
    let src = r#"
        #[hardware(sequential)]
        async fn three_stage(clk: Clock<C>, out: Out<Bits<8>, C>) {
            let mut n: Bits<8> = Bits::zero();
            loop {
                out.write(n);
                clk.tick().await;
                clk.tick().await;
                clk.tick().await;
                n = n + Bits::from_lit::<1>();
            }
        }
    "#;
    let cfg = Cfg::build(&f(src)).expect("sequential CFG builds");
    let report = cfg.fsm_report("three_stage");
    assert!(report.contains("three_stage"), "names the module: {report}");
    assert!(report.contains('3'), "reports 3 tick boundaries: {report}");
}

// ── multi-write-around-a-tick collapse detection (macro-guardrail candidate) ──
//
// Flags a combinational `Out` written on both sides of a BARE tick with a leading
// (deferred) input read shifting the pre-tick write into the pre-edge — the pattern
// the coroutine sim collapses to the last write. The three conditions are all
// necessary; validated to flag exactly the genuine cases and NOTHING in the real
// corpus (uart/rv32i/sipo_block/counter/serializers stay clean).

/// The canonical collapse: `out.write(0); tick; out.write(1)` gated by a deferred read.
#[test]
fn multi_write_flags_if_tick() {
    let src = "async fn if_tick(clk: Clock<C>, sel: In<Logic, C>, out_o: Out<Logic, C>) {
        loop {
            if sel.read() == Logic::One { out_o.write(Logic::One); clk.tick().await; }
            else { out_o.write(Logic::Zero); clk.tick().await; out_o.write(Logic::One); clk.tick().await; }
        }
    }";
    assert_eq!(multi_write_collapse(&f(src)), vec!["out_o".to_string()]);
}

/// Even an *unused* leading read triggers the collapse — it is the phase shift, not
/// the read's value, that matters.
#[test]
fn multi_write_flags_unused_leading_read() {
    let src = "async fn m(clk: Clock<C>, sel: In<Logic, C>, out: Out<Logic, C>) {
        loop { let _s = sel.read(); out.write(Logic::Zero); clk.tick().await; out.write(Logic::One); clk.tick().await; }
    }";
    assert_eq!(multi_write_collapse(&f(src)), vec!["out".to_string()]);
}

/// The SAME straddle with NO leading read does not collapse (write lands post-edge).
#[test]
fn multi_write_clean_without_leading_read() {
    let src = "async fn m(clk: Clock<C>, _sel: In<Logic, C>, out: Out<Logic, C>) {
        loop { out.write(Logic::Zero); clk.tick().await; out.write(Logic::One); clk.tick().await; }
    }";
    assert!(multi_write_collapse(&f(src)).is_empty());
}

/// `RegOut` buffers and commits at the edge, so it is never the collapsing case —
/// excluded by construction (not a combinational output).
#[test]
fn multi_write_clean_for_regout() {
    let src = "async fn m(clk: Clock<C>, sel: In<Logic, C>, out_o: RegOut<Logic, C>) {
        loop {
            if sel.read() == Logic::One { out_o.write(Logic::One); clk.tick().await; }
            else { out_o.write(Logic::Zero); clk.tick().await; out_o.write(Logic::One); clk.tick().await; }
        }
    }";
    assert!(multi_write_collapse(&f(src)).is_empty());
}

/// A single write per cycle (`counter`) is fine even with a read — one write, no straddle.
#[test]
fn multi_write_clean_for_counter() {
    let src = "async fn m(clk: Clock<C>, step: In<Bits<8>, C>, out: Out<Bits<8>, C>) {
        let mut count: Bits<8> = Bits::zero();
        loop { out.write(count); clk.tick().await; count = count + step.read(); }
    }";
    assert!(multi_write_collapse(&f(src)).is_empty());
}

/// A per-cycle serializer (`out.write(x); tick`) writes the output once per iteration.
#[test]
fn multi_write_clean_for_serializer() {
    let src = "async fn m(clk: Clock<C>, d: In<Logic, C>, out: Out<Logic, C>) {
        loop { out.write(d.read()); clk.tick().await; }
    }";
    assert!(multi_write_collapse(&f(src)).is_empty());
}
