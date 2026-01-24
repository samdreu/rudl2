// tests/counter_sim.rs
use copper_sim::Simulator;

#[test]
fn test_counter_counts() {
    let counter = Counter::<4>::new();
    let mut sim = Simulator::new(counter);
    
    sim.run_cycles(10);
    
    // Verify counter incremented
}
