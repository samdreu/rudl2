use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{HardwareExecutor, SimulationTrace, verify_with_verilator};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[derive(Clone, Copy, Debug)]
enum RxState {
    Idle,
    Start,
    Data(u8),
    Stop,
}

// Simple UART RX: 1 start bit (0), 8 data bits LSB-first, 1 stop bit (1)
#[hardware]
async fn uart_rx(
    clk: Clock<MainClk>,
    rx: Arc<Mutex<Bit>>,
    out_byte: Arc<Mutex<u8>>,
    out_valid: Arc<Mutex<Bit>>,
) {
    let mut state = RxState::Idle;
    let mut data: u8 = 0;
    let mut valid = Bit::ZERO;

    loop {
        // Output registered values
        *out_byte.lock().unwrap() = data;
        out_valid.lock().unwrap().clone_from(&valid);

        clk.tick().await;

        // Clear valid by default each cycle
        valid = Bit::ZERO;

        let rx_bit = *rx.lock().unwrap();
        match state {
            RxState::Idle => {
                if rx_bit.0 == Logic::Zero {
                    state = RxState::Start;
                }
            }
            RxState::Start => {
                // Move directly into data sampling
                state = RxState::Data(0);
            }
            RxState::Data(idx) => {
                if rx_bit.0 == Logic::One {
                    data |= 1 << idx;
                } else {
                    data &= !(1 << idx);
                }
                if idx == 7 {
                    state = RxState::Stop;
                } else {
                    state = RxState::Data(idx + 1);
                }
            }
            RxState::Stop => {
                if rx_bit.0 == Logic::One {
                    valid = Bit::ONE;
                }
                state = RxState::Idle;
            }
        }
    }
}

fn u8_to_logic_vec(val: u8) -> Vec<Logic> {
    (0..8)
        .map(|i| if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let rx = Arc::new(Mutex::new(Bit::ONE));
    let out_byte = Arc::new(Mutex::new(0u8));
    let out_valid = Arc::new(Mutex::new(Bit::ZERO));

    exec.spawn(uart_rx(
        clk.clone(),
        Arc::clone(&rx),
        Arc::clone(&out_byte),
        Arc::clone(&out_valid),
    ));

    // Send byte 0xA5 = 0b1010_0101, LSB-first: 1,0,1,0,0,1,0,1
    let mut bitstream = Vec::new();
    bitstream.extend([Logic::One, Logic::One]); // idle
    bitstream.push(Logic::Zero); // start
    bitstream.extend([
        Logic::One,
        Logic::Zero,
        Logic::One,
        Logic::Zero,
        Logic::Zero,
        Logic::One,
        Logic::Zero,
        Logic::One,
    ]);
    bitstream.push(Logic::One); // stop
    bitstream.extend([Logic::One, Logic::One]); // idle

    let mut trace = SimulationTrace::new();

    println!("=== UART RX FSM Tests (Rust Simulation) ===");
    for &bit in bitstream.iter() {
        *rx.lock().unwrap() = Bit(bit);
        exec.tick_clock(&mut clk);
        let byte = *out_byte.lock().unwrap();
        let valid = *out_valid.lock().unwrap();
        println!("cycle {} rx={:?} byte=0x{:02X} valid={:?}", clk.cycle(), bit, byte, valid.0);

        trace.add_cycle(
            clk.cycle() as usize,
            vec![("rx".to_string(), vec![bit])],
            vec![
                ("out_valid".to_string(), vec![valid.0]),
                ("out_byte".to_string(), u8_to_logic_vec(byte)),
            ],
        );
    }

    println!("\n=== Cross-Validating with Verilator ===");
    match verify_with_verilator("verilog/uart_fsm.v", "uart_fsm", &trace) {
        Ok(true) => println!("✓ Verilator verification PASSED! Rust and Verilog match!"),
        Ok(false) => println!("✗ Verilator verification FAILED!"),
        Err(e) => println!("⚠ Verilator verification error: {}", e),
    }
}
