//! P6 — X-propagation, reset, and the boundary where X stops being checkable.
//!
//! Copper's `Logic` is 3-state (`Zero`, `One`, `X`) and `Bits<N>` carries X per bit,
//! so a design can start out unknown and resolve as reset and real data arrive. This
//! file pins what X actually does end-to-end, and — just as importantly — where it
//! **cannot** be checked against hardware.
//!
//! # X splits into two regimes, and they behave completely differently
//!
//! * **Data** is X-*pessimistic*: X flows through the datapath and contaminates what
//!   it touches (`eq` on an X operand is X, not false — see
//!   `copper-core/tests/port_and_type_edge_cases.rs::eq_logic_is_x_pessimistic`).
//! * **Control** *aborts*: `as_bool()` and `as_uint()` panic on X rather than
//!   propagating it, so branching on an unknown stops the simulation instead of
//!   exploring both paths. That is a deliberate difference and it is what makes the
//!   documented panic reachable from a real design (`x_on_a_control_input_panics`).
//!
//! # Why there is no sim ≡ SV check here
//!
//! `x_cannot_be_checked_against_verilator` pins the boundary, for two independent
//! reasons either of which alone is fatal. Everything else in this file is therefore
//! a simulator-semantics test by necessity, not by choice.

mod common;

use common::{verilator_available, verilator_command};
use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct C;
impl ClockDomain for C {}

// ── The designs ──────────────────────────────────────────────────────────────

const HELD_SRC: &str = r#"
#[hardware(sequential)]
async fn held(clk: Clock<C>, en: In<Logic, C>, d: In<Bits<8>, C>, q: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::x();
    loop {
        q.write(r);
        clk.tick().await;
        if en.read() == Logic::One { r = d.read(); }
    }
}
"#;

/// A register that starts *unknown* and is loaded only while `en` is high.
///
/// The enable matters for observing the initial X at all. In a design that reloads
/// the register every cycle (`r = d.read()` unconditionally) the initial value is
/// gone before any observation point — the loop top re-runs during the post-edge
/// settle, after the load — so the X is real but invisible. Holding the register is
/// what makes it reachable.
#[hardware(sequential)]
async fn held(clk: Clock<C>, en: In<Logic, C>, d: In<Bits<8>, C>, q: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::x();
    loop {
        q.write(r);
        clk.tick().await;
        if en.read() == Logic::One {
            r = d.read();
        }
    }
}

/// Branches on a control input — the shape that aborts when the control is X.
#[hardware(sequential)]
async fn gated_counter(clk: Clock<C>, en: In<Logic, C>, q: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        q.write(r);
        clk.tick().await;
        if en.read().as_bool() {
            r = r + Bits::from_lit::<1>();
        }
    }
}

/// Starts unknown; an active-low reset drives it to a known state.
#[hardware(sequential)]
async fn resettable(
    clk: Clock<C>,
    rstn: In<Logic, C>,
    d: In<Bits<8>, C>,
    q: Out<Bits<8>, C>,
) {
    let mut r: Bits<8> = Bits::x();
    loop {
        q.write(r);
        clk.tick().await;
        if rstn.read() == Logic::Zero {
            r = Bits::zero();
        } else {
            r = d.read();
        }
    }
}

fn has_x(v: &Bits<8>) -> bool {
    v.as_array().iter().any(|l| *l == Logic::X)
}

// ── 1. an unknown register reads X, and X reaches the output ─────────────────

/// Spawn `held` and return the pieces needed to drive it.
macro_rules! held_rig {
    ($clk:ident, $exec:ident, $en:ident, $d:ident, $q:ident) => {
        let mut $clk = Clock::<C>::new();
        let mut $exec = HardwareExecutor::new();
        let ($en, en_in) = wire::<Logic, C>(Logic::Zero);
        let ($d, d_in) = wire::<Bits<8>, C>(Bits::zero());
        let (q_out, $q) = wire::<Bits<8>, C>(Bits::zero());
        let dh = q_out.dirty_handle();
        let reads = vec![en_in.wire_id(), d_in.wire_id()];
        $exec.spawn_wired(held($clk.clone(), en_in, d_in, q_out), vec![dh], reads);
    };
}

#[test]
fn an_unloaded_register_keeps_its_initial_x() {
    held_rig!(clk, exec, en, d, q);
    en.write(Logic::Zero); // never load
    d.write(Bits::<8>::from_u8(9));

    // The unknown persists across edges and is visible at the OUTPUT — X is not
    // quietly coerced to 0 on the way out of the module.
    for cycle in 0..3 {
        exec.tick_clock(&mut clk);
        assert!(has_x(&q.read()), "cycle {cycle}: an unloaded register must still read X");
    }
}

#[test]
fn loading_real_data_clears_the_unknown() {
    held_rig!(clk, exec, en, d, q);
    en.write(Logic::Zero);
    d.write(Bits::<8>::from_u8(9));
    exec.tick_clock(&mut clk);
    assert!(has_x(&q.read()));

    en.write(Logic::One); // load
    exec.tick_clock(&mut clk);
    let out = q.read();
    assert!(!has_x(&out), "loading real data must resolve the unknown");
    assert_eq!(out.as_u128(), 9);
}

#[test]
fn x_from_an_input_contaminates_the_datapath() {
    // X is pessimistic in *data*: loading an X input makes the register unknown
    // again. A simulator that resolved X to 0 here would hide a real bug.
    held_rig!(clk, exec, en, d, q);
    en.write(Logic::One);
    d.write(Bits::<8>::from_u8(5));
    exec.tick_clock(&mut clk);
    assert!(!has_x(&q.read()), "known data should have loaded");

    d.write(Bits::<8>::x()); // feed an unknown in
    exec.tick_clock(&mut clk);
    assert!(has_x(&q.read()), "an X input must make the register unknown again");
}

// ── 2. control aborts rather than propagating ────────────────────────────────

#[test]
fn x_on_a_control_input_panics() {
    // The documented `as_bool` panic, reached from a real design rather than from a
    // bare `Logic` — this is P6's "assert the panic path is reachable in-context".
    //
    // Note this is NOT X-pessimism. A pessimistic branch would have to explore both
    // arms and mark everything they touch unknown; Copper stops instead. Stopping is
    // the safer default for a simulator (an unknown condition means the testbench has
    // a bug), but it is a real semantic difference from 4-state Verilog, where
    // `if (x)` simply takes the else branch.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(|| {
        let mut clk = Clock::<C>::new();
        let mut exec = HardwareExecutor::new();
        let (en_drv, en_in) = wire::<Logic, C>(Logic::Zero);
        let (q_out, _q_obs) = wire::<Bits<8>, C>(Bits::zero());
        let dh = q_out.dirty_handle();
        let reads = vec![en_in.wire_id()];
        exec.spawn_wired(gated_counter(clk.clone(), en_in, q_out), vec![dh], reads);
        en_drv.write(Logic::X);
        exec.tick_clock(&mut clk);
    });
    std::panic::set_hook(prev);

    let payload = result.expect_err("branching on an X control input must panic");
    let msg = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        msg.contains("Cannot convert X to bool"),
        "expected the documented as_bool panic, got: {msg}"
    );
}

#[test]
fn as_uint_on_x_panics() {
    // The `Bits` counterpart, same contract.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(|| Bits::<8>::x().as_u128());
    std::panic::set_hook(prev);
    assert!(result.is_err(), "as_u128 on an unknown must panic rather than invent a value");
}

// ── 3. reset ─────────────────────────────────────────────────────────────────

#[test]
fn reset_drives_an_unknown_register_to_a_known_state() {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (rstn_drv, rstn_in) = wire::<Logic, C>(Logic::One);
    let (d_drv, d_in) = wire::<Bits<8>, C>(Bits::zero());
    let (q_out, q_obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = q_out.dirty_handle();
    let reads = vec![rstn_in.wire_id(), d_in.wire_id()];
    exec.spawn_wired(resettable(clk.clone(), rstn_in, d_in, q_out), vec![dh], reads);

    // Contaminate the register with an unknown from the datapath. (Driving X in is
    // how the register becomes observably unknown here — see
    // `an_unloaded_register_keeps_its_initial_x` for why the initialiser alone is not
    // visible through a design that reloads every cycle.)
    rstn_drv.write(Logic::One);
    d_drv.write(Bits::<8>::x());
    exec.tick_clock(&mut clk);
    assert!(has_x(&q_obs.read()), "the register should now hold an unknown");

    // Assert reset across an edge; the unknown must be gone.
    rstn_drv.write(Logic::Zero);
    d_drv.write(Bits::<8>::from_u8(0xAB));
    exec.tick_clock(&mut clk);
    let after_reset = q_obs.read();
    assert!(!has_x(&after_reset), "reset must clear the unknown");
    assert_eq!(after_reset.as_u128(), 0, "reset drives the register to zero");

    // Release reset: normal loading resumes, still with no unknowns.
    rstn_drv.write(Logic::One);
    exec.tick_clock(&mut clk);
    let loaded = q_obs.read();
    assert!(!has_x(&loaded));
    assert_eq!(loaded.as_u128(), 0xAB);
}

#[test]
fn reset_is_what_makes_a_design_verilator_checkable() {
    // Why every equivalence-tested design in the corpus resets first: the X window is
    // exactly the part that cannot be compared (see the test below). Once reset has
    // driven the state known, sim and SV are comparing the same thing again.
    //
    // This asserts the *precondition*, not the comparison: after reset there are no
    // unknowns anywhere in the observable output, so an equivalence check downstream
    // is meaningful.
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (rstn_drv, rstn_in) = wire::<Logic, C>(Logic::One);
    let (d_drv, d_in) = wire::<Bits<8>, C>(Bits::zero());
    let (q_out, q_obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = q_out.dirty_handle();
    let reads = vec![rstn_in.wire_id(), d_in.wire_id()];
    exec.spawn_wired(resettable(clk.clone(), rstn_in, d_in, q_out), vec![dh], reads);

    rstn_drv.write(Logic::Zero);
    exec.tick_clock(&mut clk);
    exec.tick_clock(&mut clk);
    rstn_drv.write(Logic::One);

    for v in 0u8..8 {
        d_drv.write(Bits::<8>::from_u8(v));
        exec.tick_clock(&mut clk);
        assert!(!has_x(&q_obs.read()), "no unknown may survive reset at cycle {v}");
    }
}

// ── 4. the boundary: X is not checkable against Verilator ────────────────────

#[test]
fn x_cannot_be_checked_against_verilator() {
    // KNOWN GAP, pinned with evidence. P6 asks for "X-pessimism end-to-end … sim ≡
    // SV". That check cannot exist, for two independent reasons either of which alone
    // is fatal:
    //
    //  1. THE X INITIALISER IS DROPPED IN TRANSPILATION. `let mut r: Bits<8> =
    //     Bits::x()` emits a bare `logic [7:0] r;` with no initial value — the
    //     "unknown" is simply not represented in the generated SystemVerilog.
    //  2. VERILATOR IS 2-STATE. Its `--x-assign` / `--x-initial` flags *assign X away*
    //     to 0, 1, or a random value; they do not model X propagation. There is no X
    //     in the reference to compare against even in principle.
    //
    // So this test pins the DIVERGENCE rather than an agreement: sim says X at cycle
    // 0, Verilator says 0. If either half is ever fixed this fails, which is the
    // signal to build the real equivalence check.
    if !verilator_available() {
        return;
    }

    // The simulator's view: never enabled, so the register stays unknown.
    held_rig!(clk, exec, en, d, q);
    en.write(Logic::Zero);
    d.write(Bits::<8>::from_u8(0));
    exec.tick_clock(&mut clk);
    assert!(has_x(&q.read()), "sim holds an unknown");

    // The transpiled view.
    let sv = copper_codegen::transpile_source(
        HELD_SRC,
        Some("held"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("transpile");
    assert!(
        !sv.contains("'x") && !sv.contains("'X"),
        "the transpiled SV now carries an X initialiser — reason 1 for this gap is \
         gone; re-check whether a real X equivalence test is now possible:\n{sv}"
    );

    let work = std::env::temp_dir().join(format!("copper_xgap_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let sv_path = work.join("held.sv");
    std::fs::write(&sv_path, &sv).unwrap();
    let tb = "#include \"Vheld.h\"\n#include \"verilated.h\"\n#include <iostream>\n\
              int main(int c, char** v) { Verilated::commandArgs(c, v); Vheld* t = new Vheld();\n\
              t->clk = 0; t->en = 0; t->d = 0; t->eval();\n\
              t->clk = 0; t->eval(); t->clk = 1; t->eval();\n\
              std::cout << (int)t->q << std::endl; return 0; }\n";
    let tb_path = work.join("tb.cpp");
    std::fs::write(&tb_path, tb).unwrap();

    let out = verilator_command()
        .current_dir(&work)
        .args([
            "--cc", "--exe", "--build", "--top-module", "held",
            "-Wno-DECLFILENAME", "-CFLAGS", "-std=c++14",
        ])
        .arg(&sv_path)
        .arg(&tb_path)
        .output()
        .expect("run verilator");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let run = std::process::Command::new(work.join("obj_dir/Vheld")).output().unwrap();
    let first = String::from_utf8_lossy(&run.stdout).trim().to_string();
    let _ = std::fs::remove_dir_all(&work);

    assert_eq!(
        first, "0",
        "Verilator no longer reports 0 for an unknown register — if it now models X, \
         reason 2 for this gap is gone and an X equivalence test may be possible"
    );
}
