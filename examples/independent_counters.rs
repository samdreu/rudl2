use copper_core::{Clock, ClockDomain, Bits};
use copper_sim::{HardwareExecutor, emit};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(function_typed)]
async fn counter(
    clk: Clock<MainClk>,
    increment: u128,
) -> Bits<8> {
    let mut count = Bits::<8>::from_u128(0);
    
    loop {
        emit!(count.clone());
        clk.tick().await;
        count = count + Bits::<8>::from_u128(increment);
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    
    // Three counters with different increments
    let out1 = exec.spawn_function_typed(Bits::<8>::from_u128(0), counter(clk.clone(), 1));
    let out2 = exec.spawn_function_typed(Bits::<8>::from_u128(0), counter(clk.clone(), 2));
    let out3 = exec.spawn_function_typed(Bits::<8>::from_u128(0), counter(clk.clone(), 5));
    
    let expected = vec![
        (1, 2, 5),   // Tick 1
        (2, 4, 10),  // Tick 2
        (3, 6, 15),  // Tick 3
        (4, 8, 20),  // Tick 4
    ];
    
    let mut all_pass = true;

    for (_i, &(exp1, exp2, exp3)) in expected.iter().enumerate() {
        exec.tick_clock(&mut clk);
        let val1 = out1.lock().unwrap().as_u128();
        let val2 = out2.lock().unwrap().as_u128();
        let val3 = out3.lock().unwrap().as_u128();
        
        let pass = val1 == exp1 && val2 == exp2 && val3 == exp3;
        let status = if pass { "✓" } else { "✗" };
        
        println!("cycle {} counters [{}, {}, {}] (expected [{}, {}, {}]) {}", 
                 clk.cycle(), val1, val2, val3, exp1, exp2, exp3, status);
        
        if !pass {
            all_pass = false;
        }
    }

    println!("\n{}", if all_pass { 
        "✓ All multi-counter tests passed!" 
    } else { 
        "✗ Some tests failed!" 
    });
    
    if !all_pass {
        std::process::exit(1);
    }
}
