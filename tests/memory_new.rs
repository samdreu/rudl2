// Integration tests for Memory latency and ready signals.
// Unit tests for the core latency logic live in copper-core/src/memory.rs.

use copper_core::{Clock, ClockDomain, Memory};

struct TestClk;
impl ClockDomain for TestClk {}

fn clk() -> Clock<TestClk> {
    Clock::<TestClk>::new()
}

#[test]
fn write_then_read_roundtrip() {
    let mut clk = clk();
    let mem = Memory::<u8, 1, 1, TestClk>::new(clk.clone(), 8);

    mem.write_port::<0>().write(5, 0xFF);
    mem.read_port::<0>().read(5);
    clk.advance(); // write commits, read captured (ReadFirst: sees 0 not 0xFF)

    assert!(mem.read_port::<0>().is_ready());
    // ReadFirst: read saw pre-write value of 0
    assert_eq!(mem.read_port::<0>().data(), 0x00);

    // Next cycle: read addr 5 again; write already committed
    mem.read_port::<0>().read(5);
    clk.advance();
    assert_eq!(mem.read_port::<0>().data(), 0xFF);
}

#[test]
fn from_contents_preloads_and_read_port_delivers_correct_value() {
    let mut clk = clk();
    let mem = Memory::<u8, 1, 0, TestClk>::from_contents(clk.clone(), vec![10, 20, 30]);

    assert_eq!(mem.size(), 3);

    for (addr, expected) in [(0, 10u8), (1, 20), (2, 30)] {
        mem.read_port::<0>().read(addr);
        clk.advance();
        assert_eq!(mem.read_port::<0>().data(), expected);
    }
}

#[test]
fn from_fn_preloads_squares() {
    let mut clk = clk();
    let mem = Memory::<u32, 1, 0, TestClk>::from_fn(clk.clone(), 4, |i| (i * i) as u32);

    assert_eq!(mem.size(), 4);
    for i in 0..4usize {
        mem.read_port::<0>().read(i);
        clk.advance();
        assert_eq!(mem.read_port::<0>().data(), (i * i) as u32);
    }
}

#[test]
fn read_not_ready_until_lat1_tick() {
    let clk = clk();
    let mem = Memory::<u8, 1, 0, TestClk>::from_contents(clk.clone(), vec![0xAB]);
    mem.read_port::<0>().read(0);
    assert!(!mem.read_port::<0>().is_ready());
}

#[test]
fn two_write_ports_last_port_index_wins_at_same_addr() {
    let mut clk = clk();
    let mem = Memory::<u8, 1, 2, TestClk>::new(clk.clone(), 4);

    mem.write_port::<0>().write(0, 0x11);
    mem.write_port::<1>().write(0, 0x22);
    clk.advance();

    mem.read_port::<0>().read(0);
    clk.advance();
    // Port 0 commits first, port 1 commits second → 0x22 wins
    assert_eq!(mem.read_port::<0>().data(), 0x22);
}
