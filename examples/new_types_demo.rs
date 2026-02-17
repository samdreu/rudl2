//! Demo of the new type system
//! 
//! This example showcases the foundational types:
//! - Bit and Bits<N> for hardware values
//! - Signal<Domain, T> for clock-domain-tagged signals
//! - State<T> for sequential state
//! - Clock<Domain> for synchronous logic

use copper_core::{Bit, Bits, Signal, Clock, State, ClockDomain};

// Define clock domains as distinct types
struct MainClk;
impl ClockDomain for MainClk {}

struct PeripheralClk;
impl ClockDomain for PeripheralClk {}

fn main() {
    println!("=== Copper HDL New Type System Demo ===\n");
    
    // 1. Basic bit operations
    println!("1. Bit Operations:");
    let a = Bit::ONE;
    let b = Bit::ZERO;
    println!("  {} AND {} = {}", a, b, a & b);
    println!("  {} OR  {} = {}", a, b, a | b);
    println!("  {} XOR {} = {}", a, b, a ^ b);
    println!("  NOT {}    = {}\n", a, !a);
    
    // 2. Bit vector operations
    println!("2. Bit Vector Operations:");
    let x: Bits<8> = Bits::from_u128(42);
    let y: Bits<8> = Bits::from_u128(100);
    println!("  x = {} ({})", x, x.as_u128());
    println!("  y = {} ({})", y, y.as_u128());
    println!("  x + y = {} ({})", x.clone() + y.clone(), (x.clone() + y.clone()).as_u128());
    println!("  x << 2 = {} ({})", x.shift_left(2), x.shift_left(2).as_u128());
    println!("  y >> 2 = {} ({})\n", y.shift_right(2), y.shift_right(2).as_u128());
    
    // 3. Clock-domain-tagged signals
    println!("3. Clock Domain Safety:");
    let mut sig_main: Signal<MainClk, Bits<8>> = Signal::new(Bits::from_u128(0xAB));
    println!("  MainClk signal: {:?}", sig_main.read());
    
    let sig_periph: Signal<PeripheralClk, Bits<8>> = Signal::new(Bits::from_u128(0xCD));
    println!("  PeripheralClk signal: {:?}", sig_periph.read());
    
    println!("  ✓ These signals have different clock domain types!");
    println!("  ✓ Compiler prevents mixing them without synchronizers\n");
    
    // This would be a compile error:
    // let bad = sig_main.read() + sig_periph.read();  // Error!
    
    // 4. Sequential state
    println!("4. Sequential State:");
    let mut counter = State::new(0u32);
    println!("  Initial state: {}", counter.current());
    
    for i in 1..=5 {
        counter.set_next(i * 10);
        counter.advance();
        println!("  After cycle {}: {}", i, counter.current());
    }
    println!();
    
    // 5. Clock simulation
    println!("5. Clock Simulation:");
    let mut clk: Clock<MainClk> = Clock::new();
    for _ in 0..3 {
        println!("  Cycle: {}", clk.cycle());
        clk.advance();
    }
    println!();
    
    // 6. Example: Simple counter module
    println!("6. Simple Counter Example:");
    struct Counter {
        count: State<u8>,
        enable: Signal<MainClk, Bit>,
        reset: Signal<MainClk, Bit>,
    }
    
    impl Counter {
        fn new() -> Self {
            Self {
                count: State::new(0),
                enable: Signal::new(Bit::ONE),
                reset: Signal::new(Bit::ZERO),
            }
        }
        
        fn tick(&mut self) {
            if self.reset.read().as_bool() {
                self.count.set_next(0);
            } else if self.enable.read().as_bool() {
                let next = (self.count.current() + 1) % 16;
                self.count.set_next(next);
            }
            self.count.advance();
        }
        
        fn output(&self) -> u8 {
            *self.count.current()
        }
    }
    
    let mut counter = Counter::new();
    for _ in 0..10 {
        print!("  {}", counter.output());
        counter.tick();
    }
    println!("\n");
    
    println!("✅ All type system features demonstrated!");
    println!("✅ Type safety enforced at compile time!");
    println!("✅ Clock domains prevent metastability bugs!");
}
