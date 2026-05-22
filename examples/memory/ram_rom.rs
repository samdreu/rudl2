use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{emit, HardwareExecutor, HardwareTest};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(function_typed)]
async fn simple_ram(
    clk: Clock<MainClk>,
    addr: Arc<Mutex<u8>>,
    we: Arc<Mutex<Bit>>,
    data_in: Arc<Mutex<u8>>,
) -> u8 {
    let mut mem = [0u8; 4];
    let mut dout: u8 = 0;

    loop {
        emit!(dout);
        clk.tick().await;

        let a   = (*addr.lock().unwrap() & 0x3) as usize;
        let w   = *we.lock().unwrap();
        let din = *data_in.lock().unwrap();

        if w == Bit::ONE {
            mem[a] = din;
        }
        dout = mem[a];
    }
}

#[hardware(function_typed)]
async fn simple_rom(
    clk: Clock<MainClk>,
    addr: Arc<Mutex<u8>>,
) -> u8 {
    let rom = [0x11u8, 0x22u8, 0x33u8, 0x44u8];
    let mut dout: u8 = 0;

    loop {
        emit!(dout);
        clk.tick().await;

        let a = (*addr.lock().unwrap() & 0x3) as usize;
        dout = rom[a];
    }
}

fn u8_to_logic_vec(val: u8) -> Vec<Logic> {
    (0..8)
        .map(|i| if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

fn u2_to_logic_vec(val: u8) -> Vec<Logic> {
    (0..2)
        .map(|i| if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let addr    = Arc::new(Mutex::new(0u8));
    let we      = Arc::new(Mutex::new(Bit::ZERO));
    let data_in = Arc::new(Mutex::new(0u8));
    let data_out = exec.spawn_function_typed(
        0u8,
        simple_ram(clk.clone(), Arc::clone(&addr), Arc::clone(&we), Arc::clone(&data_in)),
    );

    let mut ram_test = HardwareTest::new("ram")
        .with_verilog("verilog/ram.v")
        .with_waveform("waveforms/ram.vcd");

    println!("=== RAM Write ===");
    for (a, v) in [(0u8, 0xAAu8), (1, 0x55), (2, 0x0F)] {
        *addr.lock().unwrap()    = a;
        *data_in.lock().unwrap() = v;
        *we.lock().unwrap()      = Bit::ONE;
        exec.tick_clock(&mut clk);
        let dout = *data_out.lock().unwrap();
        println!("cycle {} WE=1 addr={} din=0x{:02X} dout=0x{:02X}", clk.cycle(), a, v, dout);

        let addr_logic = u2_to_logic_vec(a);
        let din_logic  = u8_to_logic_vec(v);
        let dout_logic = u8_to_logic_vec(dout);
        ram_test.record_cycle(
            clk.cycle() as usize,
            &[("addr", &addr_logic), ("we", &[Logic::One]), ("data_in", &din_logic)],
            &[("data_out", &dout_logic)],
        );
    }

    println!("\n=== RAM Read ===");
    *we.lock().unwrap() = Bit::ZERO;
    for a in [0u8, 1u8, 2u8, 3u8] {
        *addr.lock().unwrap()    = a;
        *data_in.lock().unwrap() = 0;
        exec.tick_clock(&mut clk);
        let dout = *data_out.lock().unwrap();
        println!("cycle {} WE=0 addr={} dout=0x{:02X}", clk.cycle(), a, dout);

        let addr_logic = u2_to_logic_vec(a);
        let din_logic  = u8_to_logic_vec(0);
        let dout_logic = u8_to_logic_vec(dout);
        ram_test.record_cycle(
            clk.cycle() as usize,
            &[("addr", &addr_logic), ("we", &[Logic::Zero]), ("data_in", &din_logic)],
            &[("data_out", &dout_logic)],
        );
    }

    ram_test.finish().assert_passed();

    // ── ROM ────────────────────────────────────────────────────────────────

    let addr_rom = Arc::new(Mutex::new(0u8));
    let rom_out  = exec.spawn_function_typed(
        0u8,
        simple_rom(clk.clone(), Arc::clone(&addr_rom)),
    );

    let mut rom_test = HardwareTest::new("rom")
        .with_verilog("verilog/rom.v")
        .with_waveform("waveforms/rom.vcd");

    println!("\n=== ROM Read ===");
    for a in [0u8, 1u8, 2u8, 3u8] {
        *addr_rom.lock().unwrap() = a;
        exec.tick_clock(&mut clk);
        let dout = *rom_out.lock().unwrap();
        println!("cycle {} addr={} dout=0x{:02X}", clk.cycle(), a, dout);

        let addr_logic = u2_to_logic_vec(a);
        let dout_logic = u8_to_logic_vec(dout);
        rom_test.record_cycle(
            clk.cycle() as usize,
            &[("addr", &addr_logic)],
            &[("data_out", &dout_logic)],
        );
    }

    rom_test.finish().assert_passed();
}
