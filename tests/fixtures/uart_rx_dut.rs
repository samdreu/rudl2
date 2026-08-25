// The UART receiver from `examples/uart/rx.rs`, at a bit period short enough to
// Verilate a few frames (8 cycles rather than 434 — the shape is what matters,
// and 4340 cycles per frame is trace, not coverage).
//
// The example itself only runs the simulator; this is what checks that the
// SystemVerilog it transpiles to agrees. It exercises, in one module: a
// data-dependent `while` wait, a `continue` back edge that must cost no cycle,
// three counted `for` delays, a nested counted `for` whose body does work, a
// shift register assembled across eight clock boundaries, and a register read
// after its own update in the same segment.
const CLKS_PER_BIT: usize = 8;
const CLKS_PER_HALF_BIT: usize = CLKS_PER_BIT / 2;

#[hardware(sequential)]
async fn uart_rx_dut(
    clk: Clock<MainClk>,
    rx_serial: In<Logic, MainClk>,
    rx_dv: RegOut<Logic, MainClk>,
    rx_byte: RegOut<Bits<8>, MainClk>,
) {
    loop {
        while rx_serial.read() == Logic::One {
            clk.tick().await;
        }

        for _ in 0..CLKS_PER_HALF_BIT {
            clk.tick().await;
        }
        if rx_serial.read() != Logic::Zero {
            continue;
        }

        for _ in 0..CLKS_PER_BIT {
            clk.tick().await;
        }

        let mut byte_val: Bits<8> = Bits::zero();
        for _ in 0..8 {
            byte_val = byte_val >> 1;
            if rx_serial.read() == Logic::One {
                byte_val = byte_val | Bits::from_u8(0x80);
            }
            for _ in 0..CLKS_PER_BIT {
                clk.tick().await;
            }
        }

        rx_dv.write(Logic::One);
        rx_byte.write(byte_val);
        clk.tick().await;
        rx_dv.write(Logic::Zero);
    }
}
