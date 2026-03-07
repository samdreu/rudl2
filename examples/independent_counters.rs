use copper_core::{Clock, ClockDomain, Bits};
use copper_sim::{HardwareExecutor, emit};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(function_typed)]
async fn counter(
    clk: Clock<MainClk>,
    increment: u128,
    output: Arc<Mutex<Bits<8>>>,
) -> Bits<8> {
    let mut count = Bits::<8>::from_u128(0);
    
    loop {
        emit!(output, count.clone());
        clk.tick().await;
        count = count + Bits::<8>::from_u128(increment);
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    
    let out1 = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));
    let out2 = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));
    let out3 = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));
    
    // Three counters with different increments
    exec.spawn(counter(clk.clone(), 1, Arc::clone(&out1)));
    exec.spawn(counter(clk.clone(), 2, Arc::clone(&out2)));
    exec.spawn(counter(clk.clone(), 5, Arc::clone(&out3)));
    
    let expected = vec![
        (0, 0, 0),   // Tick 1
        (1, 2, 5),   // Tick 2
        (2, 4, 10),  // Tick 3
        (3, 6, 15),  // Tick 4
    ];
    
    let mut all_pass = true;

    for (i, &(exp1, exp2, exp3)) in expected.iter().enumerate() {
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
