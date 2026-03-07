use copper_core::{Bit, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::{emit, HardwareExecutor};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// Copper version of a mux stays combinational and explicit.
// In Verilog, forgetting the else branch in an always @* block infers a latch.
fn safe_mux(sel: Bit, a: Bit, b: Bit) -> Bit {
    match sel.0 {
        Logic::One => a,
        Logic::Zero => b,
        Logic::X => Bit::X,
    }
}

// Copper version of "two writers" bug: one process owns q and encodes priority.
// In Verilog, two always blocks driving one reg is legal syntax but race-prone.
#[hardware(function_typed)]
async fn single_writer_priority(
    clk: Clock<MainClk>,
    en_a: Arc<Mutex<Bit>>,
    d_a: Arc<Mutex<Bit>>,
    en_b: Arc<Mutex<Bit>>,
    d_b: Arc<Mutex<Bit>>,
) -> Bit {
    let mut q = Bit::ZERO;
    loop {
        emit!(q);
        clk.tick().await;

        let ea = *en_a.lock().unwrap();
        let da = *d_a.lock().unwrap();
        let eb = *en_b.lock().unwrap();
        let db = *d_b.lock().unwrap();

        // Single assignment site, deterministic priority.
        q = if ea == Bit::ONE {
            da
        } else if eb == Bit::ONE {
            db
        } else {
            q
        };
    }
}

fn main() {
    println!("=== Verilog Pitfall Showcase (Copper Safe Counterparts) ===");

    println!("\n1) Latch inference from incomplete combinational assignment");
    let out0 = safe_mux(Bit::ZERO, Bit::ONE, Bit::ZERO);
    let out1 = safe_mux(Bit::ONE, Bit::ZERO, Bit::ONE);
    println!("safe_mux(sel=0, a=1, b=0) -> {:?}", out0.0);
    println!("safe_mux(sel=1, a=0, b=1) -> {:?}", out1.0);

    println!("\n2) Typo / implicit net bug");
    println!("In Copper, undeclared names are compile errors (no implicit wires).\nSee verilog/bug_implicit_net_typo.v for the Verilog version.");

    println!("\n3) Multiple drivers / race on one register");
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let en_a = Arc::new(Mutex::new(Bit::ZERO));
    let d_a = Arc::new(Mutex::new(Bit::ZERO));
    let en_b = Arc::new(Mutex::new(Bit::ZERO));
    let d_b = Arc::new(Mutex::new(Bit::ZERO));

    let q = exec.spawn_function_typed(
        Bit::ZERO,
        single_writer_priority(
            clk.clone(),
            Arc::clone(&en_a),
            Arc::clone(&d_a),
            Arc::clone(&en_b),
            Arc::clone(&d_b),
        ),
    );

    // Cycle 1: only B enabled
    *en_a.lock().unwrap() = Bit::ZERO;
    *d_a.lock().unwrap() = Bit::ONE;
    *en_b.lock().unwrap() = Bit::ONE;
    *d_b.lock().unwrap() = Bit::ONE;
    exec.tick_clock(&mut clk);
    println!("cycle {} q={:?}", clk.cycle(), q.lock().unwrap().0);

    // Cycle 2: both enabled; A wins by explicit priority
    *en_a.lock().unwrap() = Bit::ONE;
    *d_a.lock().unwrap() = Bit::ZERO;
    *en_b.lock().unwrap() = Bit::ONE;
    *d_b.lock().unwrap() = Bit::ONE;
    exec.tick_clock(&mut clk);
    println!("cycle {} q={:?} (A has priority)", clk.cycle(), q.lock().unwrap().0);

    println!("\nSee corresponding buggy Verilog modules in:");
    println!("  - verilog/bug_latch_inference.v");
    println!("  - verilog/bug_implicit_net_typo.v");
    println!("  - verilog/bug_multi_driver_race.v");
}
