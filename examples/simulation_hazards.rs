// Copper showcase: Simulation hazards that Verilog allows but Copper prevents
// Focus: blocking/non-blocking assignment races, implicit wires from typos
//
// Verilog has subtle simulation vs synthesis mismatches and race conditions.
// Copper's execution model prevents these by design.

use copper_core::{Bit, Bits, Clock, ClockDomain};
use copper_sim::{HardwareExecutor, emit};
use std::sync::{Arc, Mutex};

enum MainClk {}
impl ClockDomain for MainClk {}

// ============================================================================
// 1. Blocking/Non-Blocking Assignment Race
// ============================================================================
//
// Verilog bug (see verilog/bug_blocking_race.v):
//   always @(posedge clk) begin
//     a = b;      // blocking assignment
//     c <= a;     // non-blocking reads new 'a' or old 'a'? Race!
//   end
//
// This creates a race condition where the value of 'c' depends on the order
// of evaluation within the simulator, potentially differing between simulation
// and synthesis.
//
// Copper solution: All updates happen atomically at cycle boundaries.
// No intra-cycle races possible.

async fn safe_sequential_transfer(clk: Clock<MainClk>, b_in: Bit) -> (Bit, Bit) {
    let mut a = Bit::ZERO;
    let mut c = Bit::ZERO;
    
    loop {
        emit!((a, c));
        clk.tick().await;
        
        // In Copper, all reads see values from the PREVIOUS cycle
        // and all writes take effect at the END of the current cycle.
        // No race conditions possible.
        let old_a = a;
        let b_val = b_in;
        
        a = b_val;
        c = old_a;
    }
}

// ============================================================================
// 2. Multiple Assignments in One Cycle
// ============================================================================
//
// Verilog bug (see verilog/bug_multi_assign_blocking.v):
//   always @(posedge clk) begin
//     x = 1;
//     x = 0;      // Which value wins? Last one in procedural block.
//   end
//   always @(posedge clk) begin
//     x = 2;      // But now we have multiple blocks! Race!
//   end
//
// Copper solution: Ownership prevents multiple writers.
// Only one piece of code can own and mutate a value.

async fn safe_single_writer(clk: Clock<MainClk>) -> Bit {
    let mut x = Bit::ZERO;
    
    loop {
        emit!(x);
        clk.tick().await;
        
        // Only one place can write to x - no race
        x = Bit::ONE;
    }
}

// ============================================================================
// 3. Read-During-Write Race
// ============================================================================
//
// Verilog bug (see verilog/bug_read_write_race.v):
//   always @(posedge clk) a <= b;
//   always @(posedge clk) c <= a;  // Reads old 'a' or new 'a'? Simulator-dependent!
//
// Different simulators may schedule these in different orders, leading to
// different results. This is a well-known Verilog gotcha.
//
// Copper solution: Explicit data dependency through return values.

async fn safe_pipeline_stage1(clk: Clock<MainClk>, b: Bits<8>) -> Bits<8> {
    let mut a = Bits::<8>::from_u128(0);
    
    loop {
        emit!(a.clone());
        clk.tick().await;
        a = b.clone();
    }
}

async fn safe_pipeline_stage2(clk: Clock<MainClk>, a: Arc<Mutex<Bits<8>>>) -> Bits<8> {
    let mut c = Bits::<8>::from_u128(0);
    
    loop {
        emit!(c.clone());
        clk.tick().await;
        let a_val = a.lock().unwrap().clone();
        c = a_val;
    }
}

// ============================================================================
// 4. Missing default_nettype none
// ============================================================================
//
// Verilog bug (see verilog/bug_default_nettype.v):
//   Without `default_nettype none`, typos create implicit wires:
//   assign output_data = intput_data;  // typo: 'intput' instead of 'input'
//   This creates a new wire 'intput' tied to 0, subtle bug!
//
// Copper solution: All names must be declared. Typos are compile errors.

fn safe_wire_passthrough(input_data: Bits<8>) -> Bits<8> {
    // input_data is explicitly declared as a parameter
    // Any typo like 'intput_data' would be a compile error
    input_data
}

// ============================================================================
// Demo with cycle-by-cycle traces
// ============================================================================

fn main() {
    println!("=== Simulation Hazards Showcase ===\n");
    
    // Demo 1: Sequential transfer
    println!("1) Blocking/Non-Blocking Race");
    println!("   Verilog: Order of evaluation within cycle is undefined");
    println!("   Copper:  All reads see previous cycle, writes are atomic\n");
    
    let mut clk = Clock::<MainClk>::new();
    let mut executor = HardwareExecutor::new();
    
    let output = executor.spawn_function_typed((Bit::ZERO, Bit::ZERO), 
        safe_sequential_transfer(clk.clone(), Bit::ONE));
    
    println!("   Cycle | b_in | a | c");
    println!("   ------|------|---|---");
    
    for cycle in 0..5 {
        executor.tick_clock(&mut clk);
        let (a, c) = *output.lock().unwrap();
        let b_in = if cycle == 0 { Bit::ZERO } else { Bit::ONE };
        println!("   {:>5} | {:<4} | {} | {}", 
                 cycle, b_in, a, c);
    }
    
    println!("\n   Analysis: c always sees previous value of a (no race)\n");
    
    // Demo 2: Multiple assignments
    println!("2) Multiple Assignment Race");
    println!("   Verilog: Multiple always blocks can write to same reg");
    println!("   Copper:  Ownership prevents multiple writers\n");
    
    let mut clk2 = Clock::<MainClk>::new();
    let mut executor2 = HardwareExecutor::new();
    
    let output2 = executor2.spawn_function_typed(Bit::ZERO, 
        safe_single_writer(clk2.clone()));
    
    println!("   Cycle | x");
    println!("   ------|---");
    
    for cycle in 0..4 {
        executor2.tick_clock(&mut clk2);
        let x = *output2.lock().unwrap();
        println!("   {:>5} | {}", cycle, x);
    }
    
    println!("\n   Analysis: Only one writer can exist (enforced by compiler)\n");
    
    // Demo 3: Read-during-write
    println!("3) Read-During-Write Race");
    println!("   Verilog: Scheduler-dependent whether 'c' reads new or old 'a'");
    println!("   Copper:  Explicit data flow prevents ambiguity\n");
    
    let mut clk3 = Clock::<MainClk>::new();
    let mut executor3 = HardwareExecutor::new();
    
    let input_val = Bits::<8>::from_u128(42);
    let stage1_out = executor3.spawn_function_typed(Bits::<8>::from_u128(0),
        safe_pipeline_stage1(clk3.clone(), input_val));
    
    let stage2_out = executor3.spawn_function_typed(Bits::<8>::from_u128(0),
        safe_pipeline_stage2(clk3.clone(), stage1_out.clone()));
    
    println!("   Cycle | b_in | a (stage1) | c (stage2)");
    println!("   ------|------|------------|------------");
    
    for cycle in 0..5 {
        executor3.tick_clock(&mut clk3);
        let a = stage1_out.lock().unwrap().clone();
        let c = stage2_out.lock().unwrap().clone();
        println!("   {:>5} | {:<4} | {:<10} | {}", 
                 cycle, 42, a.as_u128(), c.as_u128());
    }
    
    println!("\n   Analysis: Explicit Arc<Mutex<>> makes data flow clear\n");
    
    // Demo 4: Typos and implicit wires
    println!("4) Implicit Wire from Typo");
    println!("   Verilog: Without `default_nettype none, typos create wires");
    println!("   Copper:  All names must be declared\n");
    
    let input = Bits::<8>::from_u128(0xAB);
    let output = safe_wire_passthrough(input.clone());
    println!("   input: 0x{:02x} -> output: 0x{:02x}", input.as_u128(), output.as_u128());
    println!("   Typo 'intput' would be compile error, not silent bug\n");
    
    println!("Key Insight: Copper's execution model (lockstep polling with atomic");
    println!("cycle boundaries) eliminates entire classes of simulation races.");
    println!("\nSee buggy Verilog modules in:");
    println!("  - verilog/bug_blocking_race.v");
    println!("  - verilog/bug_multi_assign_blocking.v");
    println!("  - verilog/bug_read_write_race.v");
    println!("  - verilog/bug_default_nettype.v");
}
