//! The UART receiver — sim ≡ transpiled SystemVerilog.
//!
//! `examples/uart/rx.rs` runs only the simulator, so until now nothing checked
//! that the receiver's transpiled form agrees with it. It is also the densest
//! control-flow module in the corpus: a data-dependent `while` wait, a `continue`
//! that must cost no cycle, four counted `for` delays (one of them nested inside a
//! `for` whose body does work), a shift register assembled across eight clock
//! boundaries, and — in `byte_val = byte_val >> 1;` followed by
//! `byte_val = byte_val | 0x80` — a register read after its own update in the same
//! segment. Causes M, N and O all meet here.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/uart_rx_dut.rs");
const SRC: &str = include_str!("fixtures/uart_rx_dut.rs");

/// One 8N1 frame as a clock-by-clock waveform: start bit, 8 data bits LSB first,
/// stop bit. Written from the protocol, not from the DUT.
fn frame(byte: u8) -> Vec<Logic> {
    let mut v = Vec::with_capacity(CLKS_PER_BIT * 10);
    v.extend(std::iter::repeat(Logic::Zero).take(CLKS_PER_BIT));
    for i in 0..8 {
        let lvl = if (byte >> i) & 1 == 1 { Logic::One } else { Logic::Zero };
        v.extend(std::iter::repeat(lvl).take(CLKS_PER_BIT));
    }
    v.extend(std::iter::repeat(Logic::One).take(CLKS_PER_BIT));
    v
}

#[test]
fn the_uart_receiver_matches_its_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("uart_rx_dut", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (s_drv, s_in) = wire::<Logic, MainClk>(Logic::One);
    let (dv_out, dv_obs) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
    let (b_out, b_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dhs = vec![dv_out.dirty_handle(), b_out.dirty_handle()];
    let reads = vec![s_in.wire_id()];
    exec.spawn_wired(uart_rx_dut(clk.clone(), s_in, dv_out, b_out), dhs, reads);

    // Idle, then three frames back to back, then idle. 0x55/0xAA catch a
    // bit-order error; 0x01 catches an off-by-one in the sampling phase.
    let sent: [u8; 3] = [0x55, 0xAA, 0x01];
    let mut wave: Vec<Logic> = std::iter::repeat(Logic::One).take(CLKS_PER_BIT).collect();
    for b in sent {
        wave.extend(frame(b));
        wave.extend(std::iter::repeat(Logic::One).take(CLKS_PER_BIT));
    }

    let mut received: Vec<u8> = Vec::new();
    let mut prev_dv = false;
    for lvl in wave {
        s_drv.write(lvl);
        exec.tick_clock(&mut clk);
        let dv = dv_obs.read();
        let by = b_obs.read();
        // Rising edge of rx_dv → one decoded byte.
        let now = dv == Logic::One;
        if now && !prev_dv {
            received.push(by.as_u128() as u8);
        }
        prev_dv = now;
        eq.record(
            &[("rx_serial", std::slice::from_ref(&lvl))],
            &[
                ("rx_dv", std::slice::from_ref(&dv)),
                ("rx_byte", &by.as_array()[..]),
            ],
            &[
                ("rx_dv", std::slice::from_ref(&dv)),
                ("rx_byte", &by.as_array()[..]),
            ],
        );
    }

    // Independent of the lowering: the receiver must decode exactly what the
    // waveform carried. A one-cycle error in the sampling phase lands on a bit
    // boundary for 0x55 and 0xAA and shows up here.
    assert_eq!(
        received, sent,
        "the SIMULATOR did not decode the frames it was sent"
    );

    eq.finish();
}
