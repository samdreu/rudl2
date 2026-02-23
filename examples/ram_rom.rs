use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{HardwareExecutor, SimulationTrace, verify_with_verilator};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware]
async fn simple_ram(
    clk: Clock<MainClk>,
    addr: Arc<Mutex<u8>>,
    we: Arc<Mutex<Bit>>,
    data_in: Arc<Mutex<u8>>,
    data_out: Arc<Mutex<u8>>,
) {
    let mut mem = [0u8; 4];
    let mut dout: u8 = 0;

    loop {
        *data_out.lock().unwrap() = dout;
        clk.tick().await;

        let a = (*addr.lock().unwrap() & 0x3) as usize;
        let w = *we.lock().unwrap();
        let din = *data_in.lock().unwrap();

        if w == Bit::ONE {
            mem[a] = din;
        }
        dout = mem[a];
    }
}

#[hardware]
async fn simple_rom(
    clk: Clock<MainClk>,
    addr: Arc<Mutex<u8>>,
    data_out: Arc<Mutex<u8>>,
) {
    let rom = [0x11u8, 0x22u8, 0x33u8, 0x44u8];
    let mut dout: u8 = 0;

    loop {
        *data_out.lock().unwrap() = dout;
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

    let addr = Arc::new(Mutex::new(0u8));
    let we = Arc::new(Mutex::new(Bit::ZERO));
    let data_in = Arc::new(Mutex::new(0u8));
    let data_out = Arc::new(Mutex::new(0u8));

    exec.spawn(simple_ram(
        clk.clone(),
        Arc::clone(&addr),
        Arc::clone(&we),
        Arc::clone(&data_in),
        Arc::clone(&data_out),
    ));

    println!("=== RAM Tests (Rust Simulation) ===");
    let mut ram_trace = SimulationTrace::new();

    // Write values
    for (a, v) in [(0u8, 0xAAu8), (1, 0x55), (2, 0x0F)] {
        *addr.lock().unwrap() = a;
        *data_in.lock().unwrap() = v;
        *we.lock().unwrap() = Bit::ONE;
        exec.tick_clock(&mut clk);
        let dout = *data_out.lock().unwrap();
        println!("cycle {} WE=1 addr={} din=0x{:02X} dout=0x{:02X}", clk.cycle(), a, v, dout);

        ram_trace.add_cycle(
            clk.cycle() as usize,
            vec![
                ("addr".to_string(), u2_to_logic_vec(a)),
                ("we".to_string(), vec![Logic::One]),
                ("data_in".to_string(), u8_to_logic_vec(v)),
            ],
            vec![("data_out".to_string(), u8_to_logic_vec(dout))],
        );
    }

    // Read values
    *we.lock().unwrap() = Bit::ZERO;
    for a in [0u8, 1u8, 2u8, 3u8] {
        *addr.lock().unwrap() = a;
        *data_in.lock().unwrap() = 0;
        exec.tick_clock(&mut clk);
        let dout = *data_out.lock().unwrap();
        println!("cycle {} WE=0 addr={} dout=0x{:02X}", clk.cycle(), a, dout);

        ram_trace.add_cycle(
            clk.cycle() as usize,
            vec![
                ("addr".to_string(), u2_to_logic_vec(a)),
                ("we".to_string(), vec![Logic::Zero]),
                ("data_in".to_string(), u8_to_logic_vec(0)),
            ],
            vec![("data_out".to_string(), u8_to_logic_vec(dout))],
        );
    }

    println!("\n=== RAM Cross-Validating with Verilator ===");
    match verify_with_verilator("verilog/ram.v", "ram", &ram_trace) {
        Ok(true) => println!("✓ RAM Verilator verification PASSED!"),
        Ok(false) => println!("✗ RAM Verilator verification FAILED!"),
        Err(e) => println!("⚠ RAM Verilator verification error: {}", e),
    }

    // ROM test
    let addr_rom = Arc::new(Mutex::new(0u8));
    let rom_out = Arc::new(Mutex::new(0u8));
    exec.spawn(simple_rom(
        clk.clone(),
        Arc::clone(&addr_rom),
        Arc::clone(&rom_out),
    ));

    let mut rom_trace = SimulationTrace::new();
    println!("\n=== ROM Tests (Rust Simulation) ===");
    for a in [0u8, 1u8, 2u8, 3u8] {
        *addr_rom.lock().unwrap() = a;
        exec.tick_clock(&mut clk);
        let dout = *rom_out.lock().unwrap();
        println!("cycle {} addr={} dout=0x{:02X}", clk.cycle(), a, dout);

        rom_trace.add_cycle(
            clk.cycle() as usize,
            vec![("addr".to_string(), u2_to_logic_vec(a))],
            vec![("data_out".to_string(), u8_to_logic_vec(dout))],
        );
    }

    println!("\n=== ROM Cross-Validating with Verilator ===");
    match verify_with_verilator("verilog/rom.v", "rom", &rom_trace) {
        Ok(true) => println!("✓ ROM Verilator verification PASSED!"),
        Ok(false) => println!("✗ ROM Verilator verification FAILED!"),
        Err(e) => println!("⚠ ROM Verilator verification error: {}", e),
    }
}
