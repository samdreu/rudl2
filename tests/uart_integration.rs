//! End-to-end integration tests for the UART system example (P5, `TODO` TESTING
//! plan). Like the CPUs, the UART only self-checked inside its own `fn main`; this
//! lifts the tx→rx loopback checks into `cargo test`.
//!
//! `spawn_uart` builds a self-contained channel whose transmitter output is wired
//! straight back to its receiver input, so a byte written to `tx_byte` (with a
//! `tx_start` pulse) should reappear on `rx_byte` after the framing + baud-sampled
//! serial round-trip (`CLKS_PER_BIT` = 434). A dropped, corrupted, or cross-wired
//! byte is a real UART/simulator bug.
//!
//! Structure: `system.rs`'s `fn main` is `#[cfg(not(test))]`, so we `include!` the
//! self-contained example at the crate root and drive its `spawn_uart` /
//! `send_and_wait` harness directly. Simulation self-check (the UART composes
//! sub-modules and is not a transpile target), not a Verilator equivalence.

#![allow(dead_code)] // the example brings a full harness; tests use a subset

include!("../examples/uart/system.rs");

/// Bytes chosen to exercise framing edges: alternating bits, all-ones, all-zeros,
/// and an asymmetric value.
const BYTES: &[u8] = &[0x37, 0xAA, 0xFF, 0x00, 0x5A, 0x81];

#[test]
fn uart_single_channel_loopback() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let uart = spawn_uart(&mut exec, clk.clone());

    for &byte in BYTES {
        let recv = send_and_wait(&mut exec, &mut clk, &uart, byte);
        assert_eq!(recv, Some(byte), "loopback of 0x{byte:02X} returned {recv:?}");
    }
}

#[test]
fn uart_dual_channel_no_crosstalk() {
    // Two channels transmit different bytes on the SAME clock cycle; each byte must
    // arrive on its own channel with no cross-contamination.
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let uart0 = spawn_uart(&mut exec, clk.clone());
    let uart1 = spawn_uart(&mut exec, clk.clone());

    for &(b0, b1) in &[(0xAB_u8, 0xCD_u8), (0x12, 0x34), (0xFF, 0x00)] {
        // Pulse both tx_start lines in the same cycle.
        uart0.tx_byte.write(Bits::from_u8(b0));
        uart0.tx_start.write(Logic::One);
        uart1.tx_byte.write(Bits::from_u8(b1));
        uart1.tx_start.write(Logic::One);
        exec.tick_clock(&mut clk);
        uart0.tx_start.write(Logic::Zero);
        uart1.tx_start.write(Logic::Zero);

        let mut recv0: Option<u8> = None;
        let mut recv1: Option<u8> = None;
        for _ in 0..CLKS_PER_BIT * 12 {
            exec.tick_clock(&mut clk);
            if uart0.rx_dv.read() == Logic::One && recv0.is_none() {
                recv0 = Some(uart0.rx_byte.read().as_u128() as u8);
            }
            if uart1.rx_dv.read() == Logic::One && recv1.is_none() {
                recv1 = Some(uart1.rx_byte.read().as_u128() as u8);
            }
        }
        while uart0.tx_busy.read() == Logic::One || uart1.tx_busy.read() == Logic::One {
            exec.tick_clock(&mut clk);
        }

        assert_eq!(recv0, Some(b0), "uart0 got {recv0:?} sending 0x{b0:02X} (uart1 sent 0x{b1:02X})");
        assert_eq!(recv1, Some(b1), "uart1 got {recv1:?} sending 0x{b1:02X} (uart0 sent 0x{b0:02X})");
    }
}
