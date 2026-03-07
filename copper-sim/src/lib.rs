use copper_core::Module;
use std::collections::HashMap;

pub mod verification;
pub use verification::{SimulationTrace, CycleData, verify_with_verilator};

pub mod executor;

pub use executor::{HardwareExecutor, ModuleInfo};

/// A helper macro to spawn a child module's future and track the parent-child relationship in the executor.
#[macro_export]
macro_rules! spawn_child {
    ($exec:expr, $parent:expr, $module_future:expr) => {{
        $exec.spawn_child(stringify!($module_future), $parent, $module_future)
    }};
    ($exec:expr, $parent:expr, $child_name:expr, $module_future:expr) => {{
        $exec.spawn_child($child_name, $parent, $module_future)
    }};
}

/// A macro for emitting values to output ports in function-typed modules.
/// This provides a clearer API for sequential modules that emit outputs each cycle.
#[macro_export]
macro_rules! emit {
    ($output:expr, $value:expr) => {{
        *$output.lock().unwrap() = $value;
    }};
}

/// A simple simulator struct that can run a hardware module and track its state over time. 
/// This is a very basic implementation and can be extended with features like waveform generation, VCD dumping, etc.
pub struct Simulator<M: Module> {
    module: M,
    cycle: u64,
    waveforms: HashMap<String, Vec<u64>>,
}

impl<M: Module> Simulator<M> {
    /// Create a new simulator with the given module. The simulator starts at cycle 0 and has an empty waveform history.
    pub fn new(module: M) -> Self {
        Simulator {
            module,
            cycle: 0,
            waveforms: HashMap::new(),
        }
    }

    /// Advance the simulation by one clock cycle. This will execute the module's logic for one cycle and update the internal state accordingly.
    pub fn clock(&mut self) {
        self.module.execute();
        self.cycle += 1;
    }

    /// Run the simulation for a specified number of cycles. This will repeatedly call the `clock` method to advance the simulation.
    pub fn run_cycles(&mut self, n: u64) {
        for _ in 0..n {
            self.clock();
        }
    }

    /// Record a signal's value at the current cycle. This can be used to track the history of signals over time for debugging and visualization purposes.
    pub fn record_signal(&mut self, name: &str, value: u64) {
        self.waveforms
            .entry(name.to_string())
            .or_insert_with(Vec::new)
            .push(value);
    }

    /// TODO: add methods for dumping waveforms, exporting VCD files, etc.
    pub fn dump_vcd(&self, filename: &str) {
        // Generate VCD (Value Change Dump) for waveform viewers
    }

    /// Get the current cycle number. This can be used for tracking the progress of the simulation and for debugging purposes.
    pub fn get_cycles(&self) -> u64 {
        self.cycle
    }

    /// Get a reference to the underlying module being simulated. This allows for inspecting the module's state and properties.
    pub fn get_module(&self) -> &M {
        &self.module
    }

    /// Get a mutable reference to the underlying module being simulated. This allows for modifying the module's state and properties during simulation.
    pub fn get_module_mut(&mut self) -> &mut M {
        &mut self.module
    }
}
