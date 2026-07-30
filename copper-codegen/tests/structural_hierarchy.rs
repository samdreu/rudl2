//! Item 4 — hierarchical clocked submodule instantiation.
//!
//! A `#[hardware(structural)]` parent instantiates clocked children on distinct
//! clock domains, wiring them with internal nets. These tests pin the transpiled
//! SystemVerilog: correct `.clk(...)` threading, internal net declarations,
//! multi-output children, and self-contained multi-module co-emission.

use copper_codegen::{transpile_source, transpile_source_hierarchy, EmitConfig};

const DUAL_CLOCK_SRC: &str = r#"
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_core::port::{wire, In, Out};
use copper_macros::hardware;

struct ClkFast;
impl ClockDomain for ClkFast {}
struct ClkSlow;
impl ClockDomain for ClkSlow {}

#[hardware(sequential)]
async fn fast_counter(
    clk: Clock<ClkFast>,
    count_out: Out<Bits<8>, ClkFast>,
    flag_out: Out<Logic, ClkFast>,
) {
    let mut count: Bits<8> = Bits::zero();
    let mut latched = Logic::Zero;
    loop {
        if count[3] == Logic::One { latched = Logic::One; }
        count_out.write(count);
        flag_out.write(latched);
        clk.tick().await;
        count = count + Bits::from_lit::<1>();
    }
}

#[hardware(synchronizer)]
async fn sync_2ff(clk: Clock<ClkSlow>, d: In<Logic, ClkFast>, q: Out<Logic, ClkSlow>) {
    let mut ff1 = Logic::Zero;
    let mut ff2 = Logic::Zero;
    loop {
        q.write(ff2);
        clk.tick().await;
        ff2 = ff1;
        ff1 = d.read();
    }
}

#[hardware(sequential)]
async fn slow_consumer(clk: Clock<ClkSlow>, flag_in: In<Logic, ClkSlow>, out: Out<Logic, ClkSlow>) {
    loop {
        out.write(flag_in.read());
        clk.tick().await;
    }
}

#[hardware(structural)]
async fn two_domain_top(
    wr_clk: Clock<ClkFast>,
    rd_clk: Clock<ClkSlow>,
    count_out: Out<Bits<8>, ClkFast>,
    flag_sync_out: Out<Logic, ClkSlow>,
) {
    let flag = wire::<Logic, ClkFast>(Logic::Zero);
    let synced = wire::<Logic, ClkSlow>(Logic::Zero);
    fast_counter(wr_clk, count_out, flag.0);
    sync_2ff(rd_clk, flag.1, synced.0);
    slow_consumer(rd_clk, synced.1, flag_sync_out);
}
"#;

#[test]
fn structural_parent_emits_clocked_instances() {
    let sv = transpile_source(DUAL_CLOCK_SRC, Some("two_domain_top"), &EmitConfig::default())
        .expect("structural parent should transpile");

    // Parent module header with both clocks as inputs and the two outputs.
    assert!(sv.contains("module two_domain_top ("), "parent module header:\n{sv}");
    assert!(sv.contains("input  logic wr_clk"), "wr_clk input:\n{sv}");
    assert!(sv.contains("input  logic rd_clk"), "rd_clk input:\n{sv}");

    // Internal nets wiring the children together are declared.
    assert!(sv.contains("logic flag;"), "internal net `flag`:\n{sv}");
    assert!(sv.contains("logic synced;"), "internal net `synced`:\n{sv}");

    // Each child is instantiated with its clock threaded to the right domain.
    assert!(sv.contains("fast_counter fast_counter_0 ("), "fast_counter instance:\n{sv}");
    assert!(sv.contains(".clk (wr_clk)"), "fast_counter on wr_clk:\n{sv}");
    assert!(sv.contains(".clk (rd_clk)"), "children on rd_clk:\n{sv}");

    // Multi-output child: fast_counter drives BOTH count_out and the flag net.
    assert!(sv.contains(".count_out (count_out)"), "count_out wired:\n{sv}");
    assert!(sv.contains(".flag_out (flag)"), "flag_out → net:\n{sv}");

    // Net driver/observer (`flag.0`/`flag.1`) both resolve to the one net `flag`.
    assert!(sv.contains(".d (flag)"), "sync_2ff.d ← flag net:\n{sv}");
    assert!(sv.contains(".q (synced)"), "sync_2ff.q → synced net:\n{sv}");
    assert!(sv.contains(".flag_in (synced)"), "consumer ← synced net:\n{sv}");

    // A structural parent has no always_ff / registers of its own.
    assert!(!sv.contains("always_ff"), "parent must have no always_ff:\n{sv}");
}

#[test]
fn hierarchy_co_emits_children_deepest_first() {
    let sv = transpile_source_hierarchy(DUAL_CLOCK_SRC, Some("two_domain_top"), &EmitConfig::default())
        .expect("hierarchy should transpile");

    // All four modules present.
    for m in ["fast_counter", "sync_2ff", "slow_consumer", "two_domain_top"] {
        assert!(sv.contains(&format!("module {m} (")), "missing module {m}:\n{sv}");
    }

    // Children are emitted before the parent (deepest-first) so the design is
    // self-contained and order-valid for any tool.
    let parent = sv.find("module two_domain_top (").unwrap();
    for child in ["fast_counter", "sync_2ff", "slow_consumer"] {
        let cpos = sv.find(&format!("module {child} (")).unwrap();
        assert!(cpos < parent, "{child} should precede the parent:\n{sv}");
    }
}

/// A parent that wires the `ClkFast` `flag` net *directly* into the `ClkSlow`
/// consumer, skipping the synchronizer — an unsynchronized clock-domain crossing.
const UNSYNCED_CROSSING_SRC: &str = r#"
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_core::port::{wire, In, Out};
use copper_macros::hardware;

struct ClkFast;
impl ClockDomain for ClkFast {}
struct ClkSlow;
impl ClockDomain for ClkSlow {}

#[hardware(sequential)]
async fn fast_counter(
    clk: Clock<ClkFast>,
    count_out: Out<Bits<8>, ClkFast>,
    flag_out: Out<Logic, ClkFast>,
) {
    let mut count: Bits<8> = Bits::zero();
    loop {
        count_out.write(count);
        flag_out.write(count[3]);
        clk.tick().await;
        count = count + Bits::from_lit::<1>();
    }
}

#[hardware(sequential)]
async fn slow_consumer(clk: Clock<ClkSlow>, flag_in: In<Logic, ClkSlow>, out: Out<Logic, ClkSlow>) {
    loop {
        out.write(flag_in.read());
        clk.tick().await;
    }
}

#[hardware(structural)]
async fn bad_top(
    wr_clk: Clock<ClkFast>,
    rd_clk: Clock<ClkSlow>,
    count_out: Out<Bits<8>, ClkFast>,
    out: Out<Logic, ClkSlow>,
) {
    let flag = wire::<Logic, ClkFast>(Logic::Zero);
    fast_counter(wr_clk, count_out, flag.0);
    slow_consumer(rd_clk, flag.1, out);
}
"#;

#[test]
fn unsynchronized_crossing_is_rejected() {
    let err = transpile_source(UNSYNCED_CROSSING_SRC, Some("bad_top"), &EmitConfig::default())
        .expect_err("an unsynchronized ClkFast→ClkSlow crossing must be rejected");
    assert!(err.contains("clock-domain crossing"), "expected a CDC error, got: {err}");
    assert!(err.contains("flag"), "error should name the offending signal: {err}");
    assert!(err.contains("synchronizer"), "error should point at the synchronizer fix: {err}");
}

#[test]
fn synchronized_crossing_is_accepted() {
    // The well-formed dual-clock design crosses ClkFast→ClkSlow *through* sync_2ff
    // and must transpile without a CDC error.
    transpile_source(DUAL_CLOCK_SRC, Some("two_domain_top"), &EmitConfig::default())
        .expect("a crossing through a synchronizer is legal");
}

#[test]
fn leaf_module_hierarchy_equals_single() {
    // A module with no submodules: hierarchy emission == single-module emission.
    let single = transpile_source(DUAL_CLOCK_SRC, Some("slow_consumer"), &EmitConfig::default()).unwrap();
    let hier = transpile_source_hierarchy(DUAL_CLOCK_SRC, Some("slow_consumer"), &EmitConfig::default()).unwrap();
    assert_eq!(single, hier);
}
