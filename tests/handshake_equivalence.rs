//! Equivalence test for `handshake` (the `while` spelling): the simulator against a
//! hand model of the source, the transpiled SystemVerilog against the simulator
//! under Verilator, and the `while` spelling against the `loop { if … { break } tick }`
//! spelling of the same machine (`handshake` in wait_loop_dut.rs), cycle for cycle.
mod common;
use common::{logic, EquivalenceTest};
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/handshake_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/handshake_dut.rs");

/// The `loop`/`break` spelling, in its own namespace (the fixture also defines
/// `waiter` and `tick_first_waiter`, which this test does not use).
#[allow(dead_code)]
mod loop_spelling {
    use super::*;
    include!("fixtures/wait_loop_dut.rs");

    // The macro makes the module private, so the two-spellings test lives in
    // here, where it can see both this `handshake` and the parent's.
    #[test]
    fn while_spelling_matches_loop_break_spelling_cycle_for_cycle() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let (req_a, req_in_a) = wire::<Logic, MainClk>(Logic::Zero);
        let (ack_a, ack_in_a) = wire::<Logic, MainClk>(Logic::Zero);
        let (done_a, obs_a) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
        let (req_b, req_in_b) = wire::<Logic, MainClk>(Logic::Zero);
        let (ack_b, ack_in_b) = wire::<Logic, MainClk>(Logic::Zero);
        let (done_b, obs_b) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
        let (dh_a, dh_b) = (done_a.dirty_handle(), done_b.dirty_handle());
        exec.spawn_wired(super::handshake(clk.clone(), req_in_a, ack_in_a, done_a), vec![dh_a], vec![]);
        exec.spawn_wired(handshake(clk.clone(), req_in_b, ack_in_b, done_b), vec![dh_b], vec![]);

        for (c, &(req, ack)) in coverage_stream().iter().enumerate() {
            req_a.write(logic(req)); ack_a.write(logic(ack));
            req_b.write(logic(req)); ack_b.write(logic(ack));
            exec.tick_clock(&mut clk);
            assert_eq!(obs_a.read(), obs_b.read(), "cycle {c}: the two spellings diverged");
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum St { Top, WaitReq, WaitAck }

fn step(st: St, n: u8, req: bool, ack: bool, done: u8) -> (St, u8, u8) {
    match st {
        St::Top => (if req { St::WaitAck } else { St::WaitReq }, n, n),
        St::WaitReq => (if req { St::WaitAck } else { St::WaitReq }, n, done),
        St::WaitAck => if ack { (St::Top, n.wrapping_add(1), done) } else { (St::WaitAck, n, done) },
    }
}

fn coverage_stream() -> Vec<(bool, bool)> {
    let s: &[(u8, u8)] = &[
        (0, 0), (0, 0), (1, 0), (0, 0), (0, 0), (0, 1),
        (1, 1), (1, 1), (1, 1),
        (0, 0), (0, 0), (1, 0), (1, 0), (1, 0), (1, 0), (1, 1),
        (1, 0), (0, 0), (0, 1), (0, 0), (0, 0),
    ];
    s.iter().map(|&(r, a)| (r == 1, a == 1)).collect()
}

#[test]
fn handshake_sim_matches_model_and_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("handshake", DUT_SRC)
        .with_hand_written_reference("tests/fixtures/reference_sv/handshake.sv");
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (req_drv, req_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (ack_drv, ack_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (done_drv, done_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = done_drv.dirty_handle();
    let reads = vec![req_in.wire_id(), ack_in.wire_id()];
    exec.spawn_wired(handshake(clk.clone(), req_in, ack_in, done_drv), vec![dh], reads);

    let (mut st, mut n, mut done) = (St::Top, 0u8, 0u8);
    for &(req, ack) in &coverage_stream() {
        req_drv.write(logic(req));
        ack_drv.write(logic(ack));
        exec.tick_clock(&mut clk);
        let (nst, nn, nd) = step(st, n, req, ack, done);
        st = nst; n = nn; done = nd;
        let expected = Bits::<8>::from_usize(done as usize);
        eq.record(
            &[("req", &[logic(req)]), ("ack", &[logic(ack)])],
            &[("done", done_obs.read().as_array().as_slice())],
            &[("done", expected.as_array().as_slice())],
        );
    }
    eq.finish();
}
