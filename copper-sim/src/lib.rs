use copper_core::Module;
use std::collections::HashMap;

pub mod verification;
pub use verification::{SimulationTrace, CycleData, verify_with_verilator};

pub mod executor;

pub use executor::{HardwareExecutor, ModuleInfo};

#[macro_export]
macro_rules! spawn_child {
    ($exec:expr, $parent:expr, $module_future:expr) => {{
        $exec.spawn_child(stringify!($module_future), $parent, $module_future)
    }};
    ($exec:expr, $parent:expr, $child_name:expr, $module_future:expr) => {{
        $exec.spawn_child($child_name, $parent, $module_future)
    }};
}

pub struct Simulator<M: Module> {
    module: M,
    cycle: u64,
    waveforms: HashMap<String, Vec<u64>>,
}

impl<M: Module> Simulator<M> {
    pub fn new(module: M) -> Self {
        Simulator {
            module,
            cycle: 0,
            waveforms: HashMap::new(),
        }
    }

    pub fn clock(&mut self) {
        self.module.execute();
        self.cycle += 1;
    }

    pub fn run_cycles(&mut self, n: u64) {
        for _ in 0..n {
            self.clock();
        }
    }

    pub fn record_signal(&mut self, name: &str, value: u64) {
        self.waveforms
            .entry(name.to_string())
            .or_insert_with(Vec::new)
            .push(value);
    }

    pub fn dump_vcd(&self, filename: &str) {
        // Generate VCD (Value Change Dump) for waveform viewers
    }

    pub fn get_cycles(&self) -> u64 {
        self.cycle
    }

    pub fn get_module(&self) -> &M {
        &self.module
    }

    pub fn get_module_mut(&mut self) -> &mut M {
        &mut self.module
    }
}
