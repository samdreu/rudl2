//! Memories with `READ_LAT` / `WRITE_LAT` greater than one: sim ≡ transpiled
//! SystemVerilog. The last construct of the `Memory` feature to reach the
//! transpiled path (`TODO` P4).
//!
//! ## What the latency is, in hardware
//!
//! A read port becomes a chain of registers. Stage `READ_LAT - 1` is the port's
//! output; stage 0 holds what the most recent edge captured:
//!
//! ```systemverilog
//! always_ff @(posedge clk) begin
//!     mem_rd0_q0 <= mem_rd0_data;   // capture
//!     mem_rd0_q1 <= mem_rd0_q0;     // shift
//! end
//! ```
//!
//! Written as one block, non-blocking assignment gives every right-hand side its
//! pre-edge value — which is exactly the simulator's "shift toward the output,
//! then capture into stage 0".
//!
//! A write port's stage 0 needs no register: it is filled by the `write()` call
//! and consumed in the same cycle. So `WRITE_LAT` yields `WRITE_LAT - 1` stage
//! registers, and the commit reads the last one.
//!
//! ## The two places latency changes an existing decision
//!
//! 1. **Which net a same-edge consumer reads.** A register update latched at the
//!    capture edge needs the value the output will hold *after* that edge. At one
//!    cycle that is the live array read; deeper, it is stage `READ_LAT - 2`, read
//!    at its pre-edge value inside `always_ff`. Getting this wrong is a silent
//!    one-cycle shift, which is why `ram_r3w1` exists — a chain deeper than two
//!    cannot be right by accident.
//! 2. **Which write the WriteFirst mux forwards.** It must be the write that
//!    COMMITS at this edge, which at `WRITE_LAT = 2` is the pipeline stage, not
//!    the freshly staged nets — those will not reach the array for another cycle.
//!    `ram_r1w2_wf` drives exactly that case.
//!
//! ## The reference model
//!
//! Written from the contract, not transcribed from `copper-core/src/memory.rs`: a
//! read presented in cycle N is captured at edge N and observable `READ_LAT - 1`
//! cycles later; a write called in cycle N commits at edge `N + WRITE_LAT - 1`;
//! ReadFirst captures before that edge's commit, WriteFirst after it.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::{Bits, Logic};
use copper_core::{Clock, ClockDomain, Memory};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/pipelined_ram_dut.rs");
const SRC: &str = include_str!("fixtures/pipelined_ram_dut.rs");

/// (we, waddr, wdata, raddr). Chosen so a write is read back at several distances
/// from its commit — before it, at the exact commit edge, and after — since a
/// wrong pipeline depth only shows up at one of those offsets.
const CASES: &[(bool, usize, u8, usize)] = &[
    (true, 3, 0xA1, 3),   // write @3, read @3 in the same cycle
    (false, 0, 0, 3),     // read @3 again
    (false, 0, 0, 3),     // and again — by now the write has landed
    (false, 0, 0, 3),
    (true, 5, 0xB2, 5),   // write @5, read @5 together
    (true, 5, 0xC3, 5),   // immediately overwrite @5 while reading it
    (false, 0, 0, 5),
    (false, 0, 0, 5),
    (false, 0, 0, 3),     // back to @3
    (true, 9, 0xD4, 0),   // write @9 while reading elsewhere
    (false, 0, 0, 9),
    (false, 0, 0, 9),
    (false, 0, 0, 9),
];

/// A memory described by its contract: latencies as delays, and a read-during-write
/// rule that says which side of the commit a capture sees.
fn expected(read_lat: usize, write_lat: usize, write_first: bool) -> Vec<u8> {
    let mut mem = [0u8; 16];
    // Value captured at each edge, indexed by edge number.
    let mut captured: Vec<u8> = Vec::new();
    // Writes scheduled to commit at a given edge.
    let mut commits: std::collections::HashMap<usize, (usize, u8)> = Default::default();
    let mut q = 0u8;
    let mut out = Vec::new();

    for (edge, &(we, wa, wd, ra)) in CASES.iter().enumerate() {
        if we {
            commits.insert(edge + write_lat - 1, (wa, wd));
        }
        let commit = commits.remove(&edge);

        // WriteFirst commits before the capture; ReadFirst after it.
        if write_first {
            if let Some((a, v)) = commit {
                mem[a] = v;
            }
            captured.push(mem[ra]);
        } else {
            captured.push(mem[ra]);
            if let Some((a, v)) = commit {
                mem[a] = v;
            }
        }

        // The output holds until the pipeline delivers; an address is presented
        // every cycle, so it delivers from edge READ_LAT - 1 onwards.
        if edge + 1 >= read_lat {
            q = captured[edge + 1 - read_lat];
        }
        out.push(q);
    }
    out
}

macro_rules! pipelined_test {
    ($name:ident, $module:ident, $rl:expr, $wl:expr, $wf:expr) => {
        #[test]
        fn $name() {
            let exp = expected($rl, $wl, $wf);
            let mut eq =
                EquivalenceTest::for_module(stringify!($module), SRC, Some(stringify!($module)));

            let mut clk = Clock::<MainClk>::new();
            let mut exec = HardwareExecutor::new();

            let (ra_drv, ra_in) = wire::<Bits<4>, MainClk>(Bits::zero());
            let (wa_drv, wa_in) = wire::<Bits<4>, MainClk>(Bits::zero());
            let (wd_drv, wd_in) = wire::<Bits<8>, MainClk>(Bits::zero());
            let (we_drv, we_in) = wire::<Logic, MainClk>(Logic::Zero);
            let (d_out, d_obs) = wire::<Bits<8>, MainClk>(Bits::zero());

            let dh = d_out.dirty_handle();
            let reads = vec![
                ra_in.wire_id(),
                wa_in.wire_id(),
                wd_in.wire_id(),
                we_in.wire_id(),
            ];
            exec.spawn_wired(
                $module(clk.clone(), ra_in, wa_in, wd_in, we_in, d_out),
                vec![dh],
                reads,
            );

            for (i, &(we, wa, wd, ra)) in CASES.iter().enumerate() {
                ra_drv.write(Bits::<4>::from_usize(ra));
                wa_drv.write(Bits::<4>::from_usize(wa));
                wd_drv.write(Bits::<8>::from_u8(wd));
                we_drv.write(Logic::from_bool(we));

                exec.tick_clock(&mut clk);

                let ra_b = Bits::<4>::from_usize(ra);
                let wa_b = Bits::<4>::from_usize(wa);
                let wd_b = Bits::<8>::from_u8(wd);
                let we_l = Logic::from_bool(we);
                let d_b = d_obs.read();
                let e_b = Bits::<8>::from_u8(exp[i]);

                eq.record(
                    &[
                        ("raddr", &ra_b.as_array()[..]),
                        ("waddr", &wa_b.as_array()[..]),
                        ("wdata", &wd_b.as_array()[..]),
                        ("we", std::slice::from_ref(&we_l)),
                    ],
                    &[("data", &d_b.as_array()[..])],
                    &[("data", &e_b.as_array()[..])],
                );
            }

            eq.finish();
        }
    };
}

pipelined_test!(read2_write2_sim_matches_transpiled_verilog, ram_r2w2, 2, 2, false);
pipelined_test!(read3_write1_sim_matches_transpiled_verilog, ram_r3w1, 3, 1, false);
pipelined_test!(read1_write2_writefirst_sim_matches_transpiled_verilog, ram_r1w2_wf, 1, 2, true);

/// The latencies must be *observable*, or the three tests above would all pass
/// against a transpiler that flattened every memory to one cycle. Pin that the
/// configurations genuinely produce different traces.
#[test]
fn the_latency_configurations_are_distinguishable() {
    let r1w1 = expected(1, 1, false);
    let r2w2 = expected(2, 2, false);
    let r3w1 = expected(3, 1, false);
    let wf = expected(1, 2, true);

    assert_ne!(r1w1, r2w2, "READ_LAT/WRITE_LAT=2 must differ from the 1-cycle memory");
    assert_ne!(r2w2, r3w1, "a third read stage must be observable");
    assert_ne!(
        wf,
        expected(1, 2, false),
        "WriteFirst must differ from ReadFirst at WRITE_LAT=2 — this is the case where \
         forwarding the freshly staged write instead of the committing one would show"
    );
}

/// Shape pins for the emitted pipelines.
#[test]
fn latency_emits_a_register_chain() {
    let emit = |m: &str| {
        copper_codegen::transpile_source(SRC, Some(m), &copper_codegen::EmitConfig::default())
            .unwrap_or_else(|e| panic!("{m} should transpile: {e}"))
    };

    // READ_LAT=2 with a same-edge consumer reads stage READ_LAT-2 = 0, so only
    // stage 0 is needed; the write chain is one stage and the commit reads it.
    let a = emit("ram_r2w2");
    assert!(
        a.contains("mem_rd0_q0 <= mem_rd0_data;") && a.contains("mem_rd0_v0 <= mem_rd0_en;"),
        "expected a read capture stage, got:\n{a}"
    );
    assert!(
        a.contains("if (mem_wr0_s1_v)") && a.contains("mem_wr0_s1_data <= mem_wr0_data;"),
        "a WRITE_LAT=2 commit must come from the pipeline stage, not the staged nets, got:\n{a}"
    );

    // READ_LAT=3 needs one more stage than READ_LAT=2.
    let b = emit("ram_r3w1");
    assert!(
        b.contains("mem_rd0_q1 <= mem_rd0_q0;"),
        "READ_LAT=3 needs a second read stage, got:\n{b}"
    );
    assert!(
        !b.contains("mem_wr0_s1_"),
        "WRITE_LAT=1 needs no write stage register, got:\n{b}"
    );

    // WriteFirst at WRITE_LAT=2 forwards the COMMITTING stage.
    let c = emit("ram_r1w2_wf");
    assert!(
        c.contains("mem_wr0_s1_v && (mem_wr0_s1_addr == mem_rd0_addr)")
            && c.contains("? mem_wr0_s1_data"),
        "the forwarding mux must take the committing stage, got:\n{c}"
    );
}
