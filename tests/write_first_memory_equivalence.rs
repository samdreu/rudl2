//! WriteFirst read-during-write: sim ≡ transpiled SystemVerilog.
//!
//! ## What WriteFirst costs, and why it was not free
//!
//! ReadFirst needed no logic at all: the write commits at the edge with a
//! non-blocking assign, so a continuous read of the array already sees the
//! pre-write contents. WriteFirst cannot work that way — the array does not hold
//! the new value until after the edge — so the read port's output net becomes a
//! forwarding mux:
//!
//! ```systemverilog
//! assign mem_rd0_data =
//!     ((mem_wr0_en && (mem_wr0_addr == mem_rd0_addr)) ? mem_wr0_data
//!                                                     : mem[mem_rd0_addr]);
//! ```
//!
//! ## The two things that can be wrong, and the two checks for them
//!
//! 1. **Forwarding could be missing** — and a WriteFirst memory that silently
//!    behaved like ReadFirst would pass every WriteFirst-only check, because the
//!    two modes agree on every cycle except a same-address read/write. So
//!    `read_first_and_write_first_actually_differ` runs both modes on identical
//!    stimulus and asserts they disagree on exactly that cycle. Without it, this
//!    file could be green against a transpiler that ignored the mode entirely.
//!
//! 2. **The mux order could be wrong.** With more than one write port aimed at one
//!    address, the forwarded value has to be the one the array would end up
//!    holding. The simulator commits writes in ascending port order (`for port in
//!    0..W`), so a later port overwrites an earlier one and the HIGHEST index wins
//!    — the rule `tests/memory_multiport_arbitration.rs` establishes at four ports,
//!    where "highest index", "second one" and "last issued" finally disagree.
//!    `ram_wf_priority` drives both ports at one address with different data, so a
//!    mux built in the other order fails immediately.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::{Bits, Logic};
use copper_core::{Clock, ClockDomain, Memory};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/write_first_ram_dut.rs");
const SRC: &str = include_str!("fixtures/write_first_ram_dut.rs");

/// (we, waddr, wdata, raddr) — cycle 2 is the read-during-write at one address.
const CASES: &[(bool, usize, u8, usize)] = &[
    (true, 3, 0xAA, 0),     // write @3; read @0 (empty)
    (false, 0, 0x00, 3),    // read @3 → 0xAA either way
    (true, 3, 0x55, 3),     // READ-DURING-WRITE @3: WriteFirst 0x55, ReadFirst 0xAA
    (false, 0, 0x00, 3),    // → 0x55 either way (the write landed)
    (true, 7, 0x11, 3),     // write elsewhere; read @3 → 0x55, no forwarding
    (true, 9, 0xFF, 9),     // read-during-write at a previously EMPTY address
    (false, 0, 0x00, 9),    // → 0xFF
];

/// Reference model. `write_first` selects which side of the edge the read sees.
fn ref_run(write_first: bool) -> Vec<u8> {
    let mut mem = [0u8; 16];
    let mut out = Vec::new();
    for &(we, wa, wd, ra) in CASES {
        let q;
        if write_first {
            if we {
                mem[wa] = wd;
            }
            q = mem[ra];
        } else {
            q = mem[ra];
            if we {
                mem[wa] = wd;
            }
        }
        out.push(q);
    }
    out
}

/// Drive `CASES` through one of the single-port DUTs.
///
/// A macro rather than a function: `#[hardware]` modules are
/// `HardwareModule<impl Future>`, which is not a `Future` and so cannot be boxed
/// behind a common function pointer. The two DUTs are byte-identical apart from
/// the builder call, so expanding the driver twice keeps them honest without
/// weakening either check.
macro_rules! single_port_test {
    ($name:ident, $module:ident, $write_first:expr) => {
        #[test]
        fn $name() {
            let expected = ref_run($write_first);
            let mut eq =
                EquivalenceTest::for_module(stringify!($module), SRC, Some(stringify!($module)));

            let mut clk = Clock::<MainClk>::new();
            let mut exec = HardwareExecutor::new();

            let (wa_drv, wa_in) = wire::<Bits<4>, MainClk>(Bits::zero());
            let (wd_drv, wd_in) = wire::<Bits<8>, MainClk>(Bits::zero());
            let (we_drv, we_in) = wire::<Logic, MainClk>(Logic::Zero);
            let (ra_drv, ra_in) = wire::<Bits<4>, MainClk>(Bits::zero());
            let (d_out, d_obs) = wire::<Bits<8>, MainClk>(Bits::zero());

            let dh = d_out.dirty_handle();
            let reads = vec![
                wa_in.wire_id(),
                wd_in.wire_id(),
                we_in.wire_id(),
                ra_in.wire_id(),
            ];
            exec.spawn_wired(
                $module(clk.clone(), wa_in, wd_in, we_in, ra_in, d_out),
                vec![dh],
                reads,
            );

            for (i, &(we, wa, wd, ra)) in CASES.iter().enumerate() {
                wa_drv.write(Bits::<4>::from_usize(wa));
                wd_drv.write(Bits::<8>::from_u8(wd));
                we_drv.write(Logic::from_bool(we));
                ra_drv.write(Bits::<4>::from_usize(ra));

                exec.tick_clock(&mut clk);

                let wa_b = Bits::<4>::from_usize(wa);
                let wd_b = Bits::<8>::from_u8(wd);
                let we_l = Logic::from_bool(we);
                let ra_b = Bits::<4>::from_usize(ra);
                let d_b = d_obs.read();
                let exp_b = Bits::<8>::from_u8(expected[i]);

                eq.record(
                    &[
                        ("waddr", &wa_b.as_array()[..]),
                        ("wdata", &wd_b.as_array()[..]),
                        ("we", std::slice::from_ref(&we_l)),
                        ("raddr", &ra_b.as_array()[..]),
                    ],
                    &[("data", &d_b.as_array()[..])],
                    &[("data", &exp_b.as_array()[..])],
                );
            }

            eq.finish();
        }
    };
}

single_port_test!(write_first_sim_matches_transpiled_verilog, ram_write_first, true);
single_port_test!(read_first_sim_matches_transpiled_verilog, ram_read_first, false);

/// The check that gives the two above their meaning: the modes must actually
/// disagree, and only on the read-during-write cycle. A transpiler that ignored
/// `.write_first()` entirely would pass both tests above and fail this one.
#[test]
fn read_first_and_write_first_actually_differ() {
    let wf = ref_run(true);
    let rf = ref_run(false);
    let differing: Vec<usize> = (0..CASES.len()).filter(|&i| wf[i] != rf[i]).collect();

    assert_eq!(
        differing,
        vec![2, 5],
        "the modes must differ on exactly the read-during-write cycles \
         (2 = same address as an existing value, 5 = same address, previously empty); \
         write_first={wf:02X?} read_first={rf:02X?}"
    );
    assert_eq!((wf[2], rf[2]), (0x55, 0xAA), "cycle 2 is the discriminating case");
}

/// Two write ports at one address, forwarded to a WriteFirst read: the value must
/// be the one the array ends up holding, i.e. the highest enabled port index.
#[test]
fn write_first_forwarding_follows_write_port_priority() {
    let mut eq = EquivalenceTest::for_module("ram_wf_priority", SRC, Some("ram_wf_priority"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (wa_drv, wa_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (d0_drv, d0_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (d1_drv, d1_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (e0_drv, e0_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (e1_drv, e1_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (ra_drv, ra_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (o_out, o_obs) = wire::<Bits<8>, MainClk>(Bits::zero());

    let dh = o_out.dirty_handle();
    let reads = vec![
        wa_in.wire_id(),
        d0_in.wire_id(),
        d1_in.wire_id(),
        e0_in.wire_id(),
        e1_in.wire_id(),
        ra_in.wire_id(),
    ];
    exec.spawn_wired(
        ram_wf_priority(clk.clone(), wa_in, d0_in, d1_in, e0_in, e1_in, ra_in, o_out),
        vec![dh],
        reads,
    );

    // (e0, e1, waddr, d0, d1, raddr)
    let cases: &[(bool, bool, usize, u8, u8, usize)] = &[
        (true, true, 5, 0xB0, 0xB1, 5),   // CONTEST at 5 → port 1 wins: 0xB1
        (false, false, 0, 0, 0, 5),       // the array must agree: 0xB1
        (true, false, 6, 0xC0, 0xC1, 6),  // only port 0 → 0xC0
        (false, true, 7, 0xD0, 0xD1, 7),  // only port 1 → 0xD1
        (true, true, 8, 0xE0, 0xE1, 5),   // contest at 8, read 5 → no forwarding: 0xB1
        (false, false, 0, 0, 0, 8),       // → 0xE1
    ];

    let mut mem = [0u8; 16];
    for &(e0, e1, wa, d0, d1, ra) in cases {
        wa_drv.write(Bits::<4>::from_usize(wa));
        d0_drv.write(Bits::<8>::from_u8(d0));
        d1_drv.write(Bits::<8>::from_u8(d1));
        e0_drv.write(Logic::from_bool(e0));
        e1_drv.write(Logic::from_bool(e1));
        ra_drv.write(Bits::<4>::from_usize(ra));

        exec.tick_clock(&mut clk);

        // Reference: writes commit in ascending port order, then the read.
        if e0 {
            mem[wa] = d0;
        }
        if e1 {
            mem[wa] = d1;
        }
        let expected = mem[ra];

        let wa_b = Bits::<4>::from_usize(wa);
        let d0_b = Bits::<8>::from_u8(d0);
        let d1_b = Bits::<8>::from_u8(d1);
        let e0_l = Logic::from_bool(e0);
        let e1_l = Logic::from_bool(e1);
        let ra_b = Bits::<4>::from_usize(ra);
        let o_b = o_obs.read();
        let exp_b = Bits::<8>::from_u8(expected);

        eq.record(
            &[
                ("waddr", &wa_b.as_array()[..]),
                ("d0", &d0_b.as_array()[..]),
                ("d1", &d1_b.as_array()[..]),
                ("e0", std::slice::from_ref(&e0_l)),
                ("e1", std::slice::from_ref(&e1_l)),
                ("raddr", &ra_b.as_array()[..]),
            ],
            &[("data", &o_b.as_array()[..])],
            &[("data", &exp_b.as_array()[..])],
        );
    }

    eq.finish();
}

/// Shape pin: ReadFirst must stay a bare array read (adding a mux there would be
/// wrong logic AND wasted silicon), and WriteFirst must forward with the highest
/// write-port index checked first.
#[test]
fn write_first_emits_a_priority_forwarding_mux() {
    let emit = |m: &str| {
        copper_codegen::transpile_source(SRC, Some(m), &copper_codegen::EmitConfig::default())
            .unwrap_or_else(|e| panic!("{m} should transpile: {e}"))
    };

    let rf = emit("ram_read_first");
    assert!(
        rf.contains("assign mem_rd0_data = mem[mem_rd0_addr];"),
        "ReadFirst needs no forwarding — the non-blocking write already gives it, got:\n{rf}"
    );

    let wf = emit("ram_write_first");
    assert!(
        wf.contains("mem_wr0_en && (mem_wr0_addr == mem_rd0_addr)")
            && wf.contains("? mem_wr0_data"),
        "WriteFirst must forward a same-address write to the read, got:\n{wf}"
    );

    // Port 1 outermost = checked first = highest index wins.
    let pri = emit("ram_wf_priority");
    let wr1 = pri.find("mem_wr1_en &&").expect("port 1 must be in the mux");
    let wr0 = pri.find("mem_wr0_en &&").expect("port 0 must be in the mux");
    assert!(
        wr1 < wr0,
        "the highest write-port index must be checked FIRST — the simulator commits \
         writes in ascending order, so port 1 is what the array ends up holding. Got:\n{pri}"
    );
}
