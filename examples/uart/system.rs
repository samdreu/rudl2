// UART system: hierarchical composition of TX and RX
//
// Demonstrates the Copper pattern for module composition: `spawn_uart` is a
// plain Rust function that creates internal wires, spawns sub-modules, and
// returns only the caller-visible interface. The TX→RX serial wire is an
// *implementation detail* — invisible to main(), which only sees:
//
//   tx_byte, tx_start, tx_busy, rx_dv, rx_byte
//
// This is hierarchical composition via Rust's ordinary abstraction mechanisms:
// no special `module` keyword or port-map syntax is needed.
//
// Test: loopback — TX serial output is wired directly to RX serial input.
// Bytes written by the caller come back out on the RX side after one 8N1 frame.

use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_core::port::{wire, In, Out};
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

const CLKS_PER_BIT: usize = 434; // 50 MHz / 115200 baud

// ── TX ────────────────────────────────────────────────────────────────────────
// Serializes one 8N1 frame (start + 8 data bits LSB-first + stop) each time
// tx_start pulses high. tx_busy is high for the duration of the frame.
async fn uart_tx(
    clk: Clock<MainClk>,
    tx_byte:   In<Bits<8>, MainClk>,
    tx_start:  In<Logic, MainClk>,
    tx_serial: Out<Logic, MainClk>,
    tx_busy:   Out<Logic, MainClk>,
) {
    loop {
        tx_serial.write(Logic::One);
        tx_busy.write(Logic::Zero);

        while tx_start.read() != Logic::One {
            clk.tick().await;
        }

        let byte = tx_byte.read();
        tx_busy.write(Logic::One);

        // Start bit
        tx_serial.write(Logic::Zero);
        for _ in 0..CLKS_PER_BIT { clk.tick().await; }

        // 8 data bits, LSB first
        for i in 0..8 {
            tx_serial.write(byte[i]);
            for _ in 0..CLKS_PER_BIT { clk.tick().await; }
        }

        // Stop bit
        tx_serial.write(Logic::One);
        for _ in 0..CLKS_PER_BIT { clk.tick().await; }
    }
}

// ── RX ────────────────────────────────────────────────────────────────────────
// Same module as examples/uart/rx.rs. Duplicated here because Cargo examples
// are standalone binaries; in the Copper module system these would be imports.
async fn uart_rx(
    clk: Clock<MainClk>,
    rx_serial: In<Logic, MainClk>,
    rx_dv:     Out<Logic, MainClk>,
    rx_byte:   Out<Bits<8>, MainClk>,
) {
    loop {
        while rx_serial.read() == Logic::One { clk.tick().await; }
        for _ in 0..CLKS_PER_BIT / 2 { clk.tick().await; }
        if rx_serial.read() != Logic::Zero { continue; }

        let mut byte_val = 0u8;
        for i in 0..8 {
            for _ in 0..CLKS_PER_BIT { clk.tick().await; }
            if rx_serial.read() == Logic::One { byte_val |= 1 << i; }
        }
        for _ in 0..CLKS_PER_BIT { clk.tick().await; }

        rx_dv.write(Logic::One);
        rx_byte.write(Bits::from_u8(byte_val));
        clk.tick().await;
        rx_dv.write(Logic::Zero);
    }
}

// ── Public interface ──────────────────────────────────────────────────────────
// The caller drives tx_byte/tx_start and observes tx_busy/rx_dv/rx_byte.
// The internal serial wire between TX and RX is not part of this type.
struct UartPorts {
    tx_byte:  Out<Bits<8>, MainClk>,
    tx_start: Out<Logic, MainClk>,
    tx_busy:  In<Logic, MainClk>,
    rx_dv:    In<Logic, MainClk>,
    rx_byte:  In<Bits<8>, MainClk>,
}

// ── Hierarchical composition ──────────────────────────────────────────────────
// Spawns uart_tx and uart_rx as sub-modules, wires them together internally,
// and returns only the caller-visible interface.
//
// The serial wire (Out<Logic, MainClk> → In<Logic, MainClk>) is created here,
// used here, and never escapes this function. From main()'s perspective the
// UART is a black box with five ports.
fn spawn_uart(exec: &mut HardwareExecutor, clk: Clock<MainClk>) -> UartPorts {
    // Internal wire: TX serial output → RX serial input.
    // Starts high (UART idle line is Logic::One).
    let (serial_out, serial_in) = wire::<Logic, MainClk>(Logic::One);

    // TX caller-side ports
    let (tx_byte_port,  tx_byte_in)  = wire::<Bits<8>, MainClk>(Bits::zero());
    let (tx_start_port, tx_start_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (tx_busy_out,   tx_busy_in)  = wire::<Logic, MainClk>(Logic::Zero);

    // RX caller-side ports
    let (rx_dv_out,   rx_dv_in)   = wire::<Logic, MainClk>(Logic::Zero);
    let (rx_byte_out, rx_byte_in) = wire::<Bits<8>, MainClk>(Bits::zero());

    let dh_serial  = serial_out.dirty_handle();
    let dh_busy    = tx_busy_out.dirty_handle();
    let dh_rx_dv   = rx_dv_out.dirty_handle();
    let dh_rx_byte = rx_byte_out.dirty_handle();

    exec.spawn_wired(
        uart_tx(clk.clone(), tx_byte_in, tx_start_in, serial_out, tx_busy_out),
        vec![dh_serial, dh_busy],
    );
    exec.spawn_wired(
        uart_rx(clk.clone(), serial_in, rx_dv_out, rx_byte_out),
        vec![dh_rx_dv, dh_rx_byte],
    );

    UartPorts {
        tx_byte:  tx_byte_port,
        tx_start: tx_start_port,
        tx_busy:  tx_busy_in,
        rx_dv:    rx_dv_in,
        rx_byte:  rx_byte_in,
    }
}

// ── Testbench ─────────────────────────────────────────────────────────────────
fn main() {
    let mut clk  = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    // Instantiate the UART subsystem. main() never touches the serial wire.
    let uart = spawn_uart(&mut exec, clk.clone());

    let test_bytes: &[(&str, u8)] = &[
        ("0x37 (ASCII '7')",   0x37),
        ("0xAA (alternating)", 0xAA),
        ("0xFF (all ones)",    0xFF),
        ("0x00 (all zeros)",   0x00),
        ("0x55 (alternating)", 0x55),
    ];

    println!("=== UART loopback (hierarchical TX+RX composition) ===");
    println!("{:<24}  {:>8}  {:>8}  {}", "byte", "sent", "received", "");

    let mut all_pass = true;

    for &(label, byte) in test_bytes {
        // Set up byte then pulse tx_start for one cycle
        uart.tx_byte.write(Bits::from_u8(byte));
        uart.tx_start.write(Logic::One);
        exec.tick_clock(&mut clk);
        uart.tx_start.write(Logic::Zero);

        // Wait for rx_dv to pulse (one full 8N1 frame = ~10 * CLKS_PER_BIT cycles).
        // Allow 12× for margin.
        let mut received: Option<u8> = None;
        for _ in 0..CLKS_PER_BIT * 12 {
            exec.tick_clock(&mut clk);
            if uart.rx_dv.read() == Logic::One && received.is_none() {
                received = Some(uart.rx_byte.read().as_u128() as u8);
            }
        }

        let pass = received == Some(byte);
        if !pass { all_pass = false; }

        println!("{:<24}  0x{:02X}      {:>8}  {}",
            label, byte,
            received.map_or("none".to_string(), |b| format!("0x{:02X}", b)),
            if pass { "✓" } else { "✗" });

        // Wait for TX to return to idle before sending the next byte
        while uart.tx_busy.read() == Logic::One {
            exec.tick_clock(&mut clk);
        }
    }

    println!("\n{}", if all_pass { "✓ All bytes received correctly." } else { "✗ Mismatch detected." });
    if !all_pass { std::process::exit(1); }
}
