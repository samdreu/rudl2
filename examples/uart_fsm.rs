use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{emit, HardwareExecutor, HardwareTest};
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
#[hardware(function_typed)]
async fn uart_rx(
    clk: Clock<MainClk>,
    rx: Arc<Mutex<Bit>>,
) -> (u8, Bit) {
    let mut state = RxState::Idle;
    let mut data: u8 = 0;
    let mut valid = Bit::ZERO;

    loop {
        emit!((data, valid));
        clk.tick().await;

        valid = Bit::ZERO;
        let rx_bit = *rx.lock().unwrap();

        match state {
            RxState::Idle => {
                if rx_bit.0 == Logic::Zero {
                    state = RxState::Start;
                }
            }
            RxState::Start => {
                state = RxState::Data(0);
            }
            RxState::Data(idx) => {
                if rx_bit.0 == Logic::One {
                    data |= 1 << idx;
                } else {
                    data &= !(1 << idx);
                }
                state = if idx == 7 { RxState::Stop } else { RxState::Data(idx + 1) };
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

    let rx  = Arc::new(Mutex::new(Bit::ONE));
    let out = exec.spawn_function_typed(
        (0u8, Bit::ZERO),
        uart_rx(clk.clone(), Arc::clone(&rx)),
    );

    // Send byte 0xA5 = 0b1010_0101, LSB-first: 1,0,1,0,0,1,0,1
    let mut bitstream = Vec::new();
    bitstream.extend([Logic::One, Logic::One]); // idle
    bitstream.push(Logic::Zero);                // start
    bitstream.extend([
        Logic::One, Logic::Zero, Logic::One, Logic::Zero,
        Logic::Zero, Logic::One, Logic::Zero, Logic::One,
    ]);
    bitstream.push(Logic::One);                 // stop
    bitstream.extend([Logic::One, Logic::One]); // idle

    let mut test = HardwareTest::new("uart_fsm")
        .with_verilog("verilog/uart_fsm.v")
        .with_waveform("waveforms/uart_fsm.vcd");

    println!("=== UART RX FSM ===");
    for &bit in bitstream.iter() {
        *rx.lock().unwrap() = Bit(bit);
        exec.tick_clock(&mut clk);
        let (byte, valid) = *out.lock().unwrap();
        println!("cycle {} rx={:?} byte=0x{:02X} valid={:?}", clk.cycle(), bit, byte, valid.0);

        let byte_logic = u8_to_logic_vec(byte);
        test.record_cycle(
            clk.cycle() as usize,
            &[("rx", &[bit])],
            &[("out_valid", &[valid.0]), ("out_byte", &byte_logic)],
        );
    }

    test.finish().assert_passed();
}
