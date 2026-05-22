use copper_core::{Bit, Clock, ClockDomain};
use copper_sim::{emit, HardwareExecutor};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// Module A: emit before first tick in a 2-tick loop.
// This keeps the old value visible for a full cycle before update propagates.
async fn before_tick(clk: Clock<MainClk>, inp: Arc<Mutex<Bit>>) -> Bit {
    let mut q = Bit::ZERO;
    loop {
        emit!(q);
        clk.tick().await;

        let d = *inp.lock().unwrap();
        q = d;
        clk.tick().await;
    }
}

// Module B: wait first, then update and emit in the same cycle.
// This makes the new value appear one cycle earlier than Module A.
async fn after_tick(clk: Clock<MainClk>, inp: Arc<Mutex<Bit>>) -> Bit {
    let mut q = Bit::ZERO;
    loop {
        clk.tick().await;

        let d = *inp.lock().unwrap();
        q = d;
        emit!(q);
        clk.tick().await;
    }
}

fn bit_to_char(bit: Bit) -> char {
    if bit == Bit::ONE {
        '1'
    } else if bit == Bit::ZERO {
        '0'
    } else {
        'X'
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let inp = Arc::new(Mutex::new(Bit::ZERO));

    let out_before = exec.spawn_function_typed(Bit::ZERO, before_tick(clk.clone(), Arc::clone(&inp)));
    let out_after = exec.spawn_function_typed(Bit::ZERO, after_tick(clk.clone(), Arc::clone(&inp)));

    let pattern = [
        Bit::ZERO,
        Bit::ONE,
        Bit::ZERO,
        Bit::ONE,
        Bit::ONE,
        Bit::ZERO,
        Bit::ONE,
    ];

    println!("Timing semantics demo: 2-tick loop (emit-before vs emit-after update)");
    println!("cycle | in | before(pre) | after(pre) | before(post) | after(post)");
    println!("------+----+-------------+------------+--------------+-----------");

    for (cycle, next_in) in pattern.iter().enumerate() {
        *inp.lock().unwrap() = *next_in;

        let before_pre = *out_before.lock().unwrap();
        let after_pre = *out_after.lock().unwrap();

        exec.tick_clock(&mut clk);

        let before_post = *out_before.lock().unwrap();
        let after_post = *out_after.lock().unwrap();

        println!(
            "{:>5} | {}  |      {}      |     {}      |      {}       |     {}",
            cycle,
            bit_to_char(*next_in),
            bit_to_char(before_pre),
            bit_to_char(after_pre),
            bit_to_char(before_post),
            bit_to_char(after_post)
        );
    }
}
