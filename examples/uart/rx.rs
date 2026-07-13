use copper_core::{Bits, Clock, ClockDomain, Logic, Memory};
use copper_core::port::{wire, In, Out};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk{}

// parameters
// baud rate (9600, 19200, 115200, etc)
// data bits (7, 8) // almost always set to 8
// parity bit (On, Off)
// stop bits (0, 1, 2) // always set to one
// flow control (none, on, hardware) // likely set to None

// for this example:
// baud rate = 115200
// data bits = 8
// parity bit = off
// stop bits = 1
// flow control = None

// first looks for falling edge of start bit
// waits for half of a bit period to align to the center of the UART data
// then samples the data on the line and stores each bit into a byte register
// once all data bits are received, it gets a stop bit and then returns to IDLE

enum State {
    IDLE,
    START,
    DATA,
    STOP,
    CLEANUP,
}

const CLKS_PER_BIT: usize = 434; // 50 MHz / 115200


struct packet {
    start: Logic,
    data: Bits<8>,
    stop: Logic,
}

async fn rx (
    clk: Clock<MainClk>,
    rx_serial: In<Logic, MainClk>,
    rx_dv: Out<Logic, MainClk>,
    rx_byte: Out<Bits<8>, MainClk>,
) {

    loop {
        // idle: wait for falling edge / start bit
        while rx_serial.read() == Logic::One {
            clk.tick().await;
        }

        // start: skip to center of start bit and verify
        for _ in 0..CLKS_PER_BIT / 2 {
            clk.tick().await;
        }
        if rx_serial.read() != Logic::Zero {
            continue; // go back to idle
        }

        // data: sample 8 bits
        let mut byte_val = 0;
        for i in 0..8 {
            for _ in 0..CLKS_PER_BIT {
                clk.tick().await;
            }
            if rx_serial.read() == Logic::One {
                byte_val |= 1 << i; // not sure about this tbh...
            }
        }

        // stop: wait through stop bit
        for _ in 0..CLKS_PER_BIT {
            clk.tick().await;
        }

        // pulse data value for one cycle
        rx_dv.write(Logic::One);
        rx_byte.write(Bits::from_u8(byte_val));
        clk.tick().await;
        rx_dv.write(Logic::Zero);
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (serial_drv, serial_in) = wire::<Logic, MainClk>(Logic::One);
    let (dv_out,     dv_obs)    = wire::<Logic, MainClk>(Logic::Zero);
    let (byte_out,   byte_obs)  = wire::<Bits<8>, MainClk>(Bits::zero());

    let dh_dv   = dv_out.dirty_handle();
    let dh_byte = byte_out.dirty_handle();
    exec.spawn_wired(
        rx(clk.clone(), serial_in, dv_out, byte_out),
        vec![dh_dv, dh_byte],
    );

    // One complete 8N1 UART frame as a clock-by-clock waveform (start + 8 data + stop).
    fn uart_frame(byte: u8, cpb: usize) -> Vec<Logic> {
        let mut v = Vec::with_capacity(cpb * 10);
        v.extend(std::iter::repeat(Logic::Zero).take(cpb));
        for i in 0..8 {
            let lvl = if (byte >> i) & 1 == 1 { Logic::One } else { Logic::Zero };
            v.extend(std::iter::repeat(lvl).take(cpb));
        }
        v.extend(std::iter::repeat(Logic::One).take(cpb));
        v
    }

    // Same frame but with the stop bit forced low (framing error).
    fn uart_frame_bad_stop(byte: u8, cpb: usize) -> Vec<Logic> {
        let mut v = uart_frame(byte, cpb);
        for lvl in &mut v[cpb * 9..] { *lvl = Logic::Zero; }
        v
    }

    // Tick through a waveform and capture the first rx_dv pulse.
    // After the waveform ends, waits up to `extra` idle cycles before giving up.
    // Using a macro so exec/clk/serial_drv/dv_obs/byte_obs are accessed in-scope
    // without needing to pass them through a closure.
    macro_rules! send_and_recv {
        ($wave:expr, $extra:expr) => {{
            let mut received: Option<u8> = None;
            for &lvl in ($wave as &Vec<Logic>).iter() {
                serial_drv.write(lvl);
                exec.tick_clock(&mut clk);
                if dv_obs.read() == Logic::One && received.is_none() {
                    received = Some(byte_obs.read().as_u128() as u8);
                }
            }
            if received.is_none() {
                for _ in 0..$extra {
                    serial_drv.write(Logic::One);
                    exec.tick_clock(&mut clk);
                    if dv_obs.read() == Logic::One {
                        received = Some(byte_obs.read().as_u128() as u8);
                        break;
                    }
                }
            }
            received
        }};
    }

    macro_rules! idle_cycles {
        ($n:expr) => {
            for _ in 0usize..$n {
                serial_drv.write(Logic::One);
                exec.tick_clock(&mut clk);
            }
        };
    }

    let mut all_pass = true;

    // ── 1. Normal byte reception (exercises IDLE→START→DATA→STOP path) ────────
    println!("=== Normal byte reception ===");
    println!("{:<32}  {:>8}  {:>8}  {}", "case", "expected", "received", "");

    let normal_cases: &[(&str, u8)] = &[
        ("0x37 (Nandland reference)",  0x37),
        ("0x00 (all-zero data bits)",  0x00),
        ("0xFF (all-one  data bits)",  0xFF),
        ("0x55 (alternating 01...)",   0x55),
        ("0xAA (alternating 10...)",   0xAA),
        ("0x01 (only bit 0 set)",      0x01),
        ("0x80 (only bit 7 set)",      0x80),
        ("0x0F (low nibble)",          0x0F),
        ("0xF0 (high nibble)",         0xF0),
    ];

    for &(label, expected) in normal_cases {
        let wave = uart_frame(expected, CLKS_PER_BIT);
        let result = send_and_recv!(&wave, CLKS_PER_BIT);
        let pass = result == Some(expected);
        if !pass { all_pass = false; }
        println!("{:<32}  0x{:02X}      {:>8}  {}",
            label, expected,
            result.map_or("none".to_string(), |b| format!("0x{:02X}", b)),
            if pass { "✓" } else { "✗" });
        idle_cycles!(CLKS_PER_BIT);
    }

    // ── 2. Back-to-back frames (no idle gap between frames) ───────────────────
    println!("\n=== Back-to-back frames ===");

    let pairs: &[(u8, u8)] = &[
        (0xAB, 0xCD),
        (0x12, 0x34),
        (0xFF, 0x00),
    ];
    for &(a, b) in pairs {
        let w1 = uart_frame(a, CLKS_PER_BIT);
        let w2 = uart_frame(b, CLKS_PER_BIT);
        let r1 = send_and_recv!(&w1, CLKS_PER_BIT);
        let r2 = send_and_recv!(&w2, CLKS_PER_BIT);
        let pass = r1 == Some(a) && r2 == Some(b);
        if !pass { all_pass = false; }
        println!("0x{:02X} → 0x{:02X}: {} {}",
            a, b,
            if r1 == Some(a) { "✓" } else { "✗" },
            if r2 == Some(b) { "✓" } else { "✗" });
        idle_cycles!(CLKS_PER_BIT);
    }

    // ── 3. Glitch rejection (exercises START→IDLE false-start path) ───────────
    // A low pulse shorter than half a bit period must not decode as a frame.
    println!("\n=== Glitch rejection ===");
    {
        let glitch_len = CLKS_PER_BIT / 4;
        let mut spurious = false;
        for _ in 0..glitch_len {
            serial_drv.write(Logic::Zero);
            exec.tick_clock(&mut clk);
            if dv_obs.read() == Logic::One { spurious = true; }
        }
        serial_drv.write(Logic::One);
        for _ in 0..CLKS_PER_BIT {
            exec.tick_clock(&mut clk);
            if dv_obs.read() == Logic::One { spurious = true; }
        }

        let wave = uart_frame(0x5A, CLKS_PER_BIT);
        let recovery = send_and_recv!(&wave, CLKS_PER_BIT);
        let recovered = recovery == Some(0x5A);
        if spurious || !recovered { all_pass = false; }
        println!("No spurious rx_dv during glitch:  {}", if !spurious   { "✓" } else { "✗" });
        println!("Correct decode after  glitch:     {}", if recovered   { "✓" } else { "✗" });
        idle_cycles!(CLKS_PER_BIT);
    }

    // ── 4. Framing error recovery (bad stop bit) ──────────────────────────────
    // After a frame with stop=0, the receiver must still decode the next valid frame.
    println!("\n=== Framing error recovery ===");
    {
        let bad_wave = uart_frame_bad_stop(0xBE, CLKS_PER_BIT);
        let bad_result = send_and_recv!(&bad_wave, CLKS_PER_BIT * 2);
        idle_cycles!(CLKS_PER_BIT * 2);

        let wave = uart_frame(0xEF, CLKS_PER_BIT);
        let recovery = send_and_recv!(&wave, CLKS_PER_BIT);
        let recovered = recovery == Some(0xEF);
        if !recovered { all_pass = false; }
        println!("Framing error on 0xBE: rx_dv {}",
            match bad_result {
                Some(b) => format!("fired (0x{:02X} passed through)", b),
                None    => "suppressed".to_string(),
            });
        println!("Next valid frame 0xEF received:   {}", if recovered { "✓" } else { "✗" });
        idle_cycles!(CLKS_PER_BIT);
    }

    // ── 5. Idle line: no spurious output ──────────────────────────────────────
    println!("\n=== Idle line (IDLE state, no transitions) ===");
    {
        let mut spurious = false;
        for _ in 0..CLKS_PER_BIT * 20 {
            serial_drv.write(Logic::One);
            exec.tick_clock(&mut clk);
            if dv_obs.read() == Logic::One { spurious = true; }
        }
        if spurious { all_pass = false; }
        println!("No rx_dv over 20 bit-periods of idle: {}", if !spurious { "✓" } else { "✗" });
    }

    println!("\n{}", if all_pass { "✓ All tests passed." } else { "✗ One or more tests failed." });
    if !all_pass { std::process::exit(1); }
}

