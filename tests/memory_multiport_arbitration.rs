//! P4 — multi-read-port / multi-write-port arbitration at scale.
//!
//! `copper-core`'s in-crate tests cover the two-port case
//! (`two_write_ports_last_wins_at_same_addr`, `two_read_ports_independent`). Two
//! ports is the smallest configuration in which a priority rule is even visible, and
//! it cannot distinguish "the highest index wins" from "the second one wins" or "the
//! last one written in source order wins" — three different rules that agree on two
//! ports and disagree on four.
//!
//! These tests use 4R/4W and vary the source order deliberately, so the rule is
//! pinned as *port index* priority rather than anything that merely correlates with
//! it at small scale.
//!
//! The rule itself (`advance_write_pipelines`): ports commit in index order `0..W`,
//! each doing `data[addr] = value`, so on a contested address the **highest port
//! index wins**. In hardware that is a priority-encoded write port, and the encoding
//! is the port index.

use copper_core::memory::Memory;
use copper_core::{Clock, ClockDomain};

struct MemClk;
impl ClockDomain for MemClk {}

const SIZE: usize = 16;

/// Read `addr` on read port 0 and return the value, costing one posedge.
macro_rules! read_back {
    ($mem:expr, $clk:expr, $port:literal, $addr:expr) => {{
        $mem.read_port::<$port>().read($addr);
        $clk.advance();
        $mem.read_port::<$port>().data()
    }};
}

#[test]
fn highest_write_port_index_wins_at_a_contested_address() {
    let mut clk = Clock::<MemClk>::new();
    let mem = Memory::<u8, 4, 4, MemClk>::new(clk.clone(), SIZE);

    // All four ports write the SAME address in one cycle, issued in ASCENDING order.
    mem.write_port::<0>().write(5, 0xA0);
    mem.write_port::<1>().write(5, 0xA1);
    mem.write_port::<2>().write(5, 0xA2);
    mem.write_port::<3>().write(5, 0xA3);
    clk.advance();

    assert_eq!(
        read_back!(mem, clk, 0, 5),
        0xA3,
        "the highest port index must win a contested address"
    );
}

#[test]
fn the_winner_is_the_port_index_not_the_source_order() {
    let mut clk = Clock::<MemClk>::new();
    let mem = Memory::<u8, 4, 4, MemClk>::new(clk.clone(), SIZE);

    // Same contest, but issued in DESCENDING source order. If the rule were "last
    // write call wins" this would yield 0xB0; if it is port-index priority it still
    // yields the port-3 value. Two ports cannot tell these apart — four can.
    mem.write_port::<3>().write(6, 0xB3);
    mem.write_port::<2>().write(6, 0xB2);
    mem.write_port::<1>().write(6, 0xB1);
    mem.write_port::<0>().write(6, 0xB0);
    clk.advance();

    assert_eq!(
        read_back!(mem, clk, 0, 6),
        0xB3,
        "priority must follow the PORT INDEX, not the order the writes were issued"
    );
}

#[test]
fn a_partial_contest_is_won_by_the_highest_contender_present() {
    let mut clk = Clock::<MemClk>::new();
    let mem = Memory::<u8, 4, 4, MemClk>::new(clk.clone(), SIZE);

    // Only ports 0 and 2 contest; port 3 is idle. The winner should be 2, not "the
    // last port index that exists".
    mem.write_port::<0>().write(7, 0xC0);
    mem.write_port::<2>().write(7, 0xC2);
    clk.advance();

    assert_eq!(read_back!(mem, clk, 0, 7), 0xC2);
}

#[test]
fn uncontested_ports_all_land_in_the_same_cycle() {
    let mut clk = Clock::<MemClk>::new();
    let mem = Memory::<u8, 4, 4, MemClk>::new(clk.clone(), SIZE);

    // Four ports, four DIFFERENT addresses, one cycle. Arbitration must only apply
    // where addresses collide — a priority rule that serialised unrelated writes
    // would be a functional bug, not just a slow one.
    mem.write_port::<0>().write(0, 0xD0);
    mem.write_port::<1>().write(1, 0xD1);
    mem.write_port::<2>().write(2, 0xD2);
    mem.write_port::<3>().write(3, 0xD3);
    clk.advance();

    for (addr, expected) in [(0usize, 0xD0u8), (1, 0xD1), (2, 0xD2), (3, 0xD3)] {
        assert_eq!(read_back!(mem, clk, 0, addr), expected, "addr {addr} lost its write");
    }
}

#[test]
fn four_read_ports_are_independent_in_one_cycle() {
    let mut clk = Clock::<MemClk>::new();
    let mem = Memory::<u8, 4, 4, MemClk>::from_fn(clk.clone(), SIZE, |i| (i as u8) * 3);

    // Every read port addresses a different location in the same cycle; each must
    // deliver its own value, not another port's.
    mem.read_port::<0>().read(1);
    mem.read_port::<1>().read(4);
    mem.read_port::<2>().read(9);
    mem.read_port::<3>().read(12);
    clk.advance();

    assert_eq!(mem.read_port::<0>().data(), 3);
    assert_eq!(mem.read_port::<1>().data(), 12);
    assert_eq!(mem.read_port::<2>().data(), 27);
    assert_eq!(mem.read_port::<3>().data(), 36);
}

#[test]
fn read_ports_all_observe_the_arbitration_winner() {
    let mut clk = Clock::<MemClk>::new();
    let mem = Memory::<u8, 4, 4, MemClk>::new(clk.clone(), SIZE);

    mem.write_port::<1>().write(8, 0xE1);
    mem.write_port::<3>().write(8, 0xE3);
    clk.advance();

    // A contested write resolves to ONE value in the array; every read port must see
    // that same value. Divergent per-port views would mean the ports are not reading
    // one memory.
    mem.read_port::<0>().read(8);
    mem.read_port::<1>().read(8);
    mem.read_port::<2>().read(8);
    mem.read_port::<3>().read(8);
    clk.advance();

    assert_eq!(mem.read_port::<0>().data(), 0xE3);
    assert_eq!(mem.read_port::<1>().data(), 0xE3);
    assert_eq!(mem.read_port::<2>().data(), 0xE3);
    assert_eq!(mem.read_port::<3>().data(), 0xE3);
}

#[test]
fn arbitration_holds_under_write_latency() {
    let mut clk = Clock::<MemClk>::new();
    // WRITE_LAT = 2: the contest is resolved when the writes COMMIT, two posedges
    // later, not when they are issued.
    let mem = Memory::<u8, 4, 4, MemClk, 1, 2>::new(clk.clone(), SIZE);

    mem.write_port::<0>().write(10, 0xF0);
    mem.write_port::<3>().write(10, 0xF3);
    clk.advance(); // stage 0 -> stage 1
    clk.advance(); // commit

    assert_eq!(
        read_back!(mem, clk, 0, 10),
        0xF3,
        "port-index priority must still hold when writes are pipelined"
    );
}

// ── Out-of-range addressing (P4) ─────────────────────────────────────────────

/// Out-of-range addressing **panics deliberately**, with a message naming the port,
/// the address and the size — and it panics at the `read()`/`write()` call, not when
/// the pipeline commits at a clock edge.
///
/// The alternatives were returning `X` (what SystemVerilog does on an out-of-range
/// read) or wrapping (what real address decoding does when the index has fewer bits
/// than you supplied). Both silently substitute a value, turning an addressing bug
/// into a wrong-answer bug somewhere downstream. Copper has no transpiled `Memory`
/// path to be faithful to — see
/// `copper-codegen/tests/unsupported_constructs.rs::memory_is_not_transpilable` — so
/// there is no sim ≡ synth pressure here, and the diagnostic is worth more than the
/// fidelity. Revisit if `Memory` ever gains a transpiled path.
///
/// Before this was made deliberate it still panicked, but incidentally: a raw
/// `index out of bounds: the len is 4 but the index is 104` from inside
/// `advance_write_pipelines`, at the clock edge, naming neither the memory nor the
/// port. These tests pin the contract, not just the crash.
mod out_of_range {
    use super::*;

    fn panic_message(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let payload = std::panic::catch_unwind(f).expect_err("expected an out-of-range panic");
        std::panic::set_hook(prev);
        payload
            .downcast_ref::<String>()
            .map(|s| s.clone())
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    }

    #[test]
    fn a_write_past_the_end_names_the_port_address_and_size() {
        let msg = panic_message(|| {
            let clk = Clock::<MemClk>::new();
            let mem = Memory::<u8, 1, 1, MemClk>::new(clk.clone(), 4);
            mem.write_port::<0>().write(104, 0xAA);
        });
        assert!(msg.contains("write port 0"), "should name the port kind and index: {msg}");
        assert!(msg.contains("104"), "should name the offending address: {msg}");
        assert!(msg.contains('4') && msg.contains("0..=3"), "should name the size and range: {msg}");
    }

    #[test]
    fn a_read_past_the_end_names_the_port_address_and_size() {
        let msg = panic_message(|| {
            let clk = Clock::<MemClk>::new();
            let mem = Memory::<u8, 1, 1, MemClk>::new(clk.clone(), 4);
            mem.read_port::<0>().read(9);
        });
        assert!(msg.contains("read port 0"), "should name the port kind and index: {msg}");
        assert!(msg.contains('9'), "should name the offending address: {msg}");
    }

    #[test]
    fn the_last_valid_address_is_accepted() {
        // The boundary itself must not be rejected — an off-by-one in the check
        // would make the top entry unusable.
        let mut clk = Clock::<MemClk>::new();
        let mem = Memory::<u8, 1, 1, MemClk>::new(clk.clone(), 4);
        mem.write_port::<0>().write(3, 0x5A);
        clk.advance();
        assert_eq!(read_back!(mem, clk, 0, 3), 0x5A);
    }

    #[test]
    fn it_panics_at_the_call_not_at_the_clock_edge() {
        // The whole point of checking at the call site: no `advance()` here. If the
        // check moved back to commit time this would not panic at all.
        let msg = panic_message(|| {
            let clk = Clock::<MemClk>::new();
            let mem = Memory::<u8, 1, 1, MemClk>::new(clk.clone(), 4);
            mem.write_port::<0>().write(4, 0x01);
            // deliberately no clk.advance()
        });
        assert!(msg.contains("out of range"), "{msg}");
    }
}
