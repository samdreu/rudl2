use copper_core::Module;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

pub mod verification;
pub use verification::{SimulationTrace, CycleData, verify_with_verilator};

pub mod testing;
pub use testing::{HardwareTest, TestResult, make_cycle};

pub mod executor;

pub use executor::{HardwareExecutor, ModuleInfo};

#[doc(hidden)]
pub mod synced_read;

/// Which phase of the clock cycle `tick_clock` is currently executing.
/// Set by the executor before each `poll_tasks()` call so that phase-aware futures
/// (`PreEdgeBarrier`) can determine whether to block or proceed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollPhase {
    PreEdge,
    PostEdge,
}

thread_local! {
    static CURRENT_EMIT_TARGET: RefCell<Option<Arc<dyn Any + Send + Sync>>> = RefCell::new(None);
    /// Set to true by `emit_to_current` whenever a value is written.
    /// Reset to false by `push_emit_target` before each task poll so each poll starts clean.
    /// Read by the executor after each poll to decide whether another delta cycle is needed.
    static CURRENT_EMIT_DIRTY: RefCell<bool> = RefCell::new(false);
    /// Current clock phase set by `tick_clock` before each settle pass.
    static POLL_PHASE: RefCell<PollPhase> = RefCell::new(PollPhase::PreEdge);
}

pub(crate) fn set_poll_phase(phase: PollPhase) {
    POLL_PHASE.with(|cell| *cell.borrow_mut() = phase);
}

fn is_pre_edge() -> bool {
    POLL_PHASE.with(|cell| *cell.borrow() == PollPhase::PreEdge)
}

pub(crate) struct EmitTargetGuard {
    previous: Option<Arc<dyn Any + Send + Sync>>,
}

impl Drop for EmitTargetGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT_EMIT_TARGET.with(|cell| {
            *cell.borrow_mut() = previous;
        });
    }
}

pub(crate) fn push_emit_target(target: Option<Arc<dyn Any + Send + Sync>>) -> EmitTargetGuard {
    // Clear the dirty flag so the upcoming poll starts with a clean slate.
    CURRENT_EMIT_DIRTY.with(|cell| *cell.borrow_mut() = false);

    let previous = CURRENT_EMIT_TARGET.with(|cell| {
        let mut slot = cell.borrow_mut();
        std::mem::replace(&mut *slot, target)
    });
    EmitTargetGuard { previous }
}

/// Read and reset the dirty flag that `emit_to_current` sets.
/// Returns true if the most recent task poll called `emit!` at least once.
pub(crate) fn take_emit_dirty() -> bool {
    CURRENT_EMIT_DIRTY.with(|cell| {
        let dirty = *cell.borrow();
        *cell.borrow_mut() = false;
        dirty
    })
}

pub fn emit_to_current<T>(value: T)
where
    T: PartialEq + Send + 'static,
{
    CURRENT_EMIT_TARGET.with(|cell| {
        let target = cell
            .borrow()
            .as_ref()
            .cloned()
            .expect("emit!(value) called without a bound function-typed output");

        let typed = Arc::downcast::<Mutex<T>>(target)
            .expect("emit!(value) type mismatch for currently bound function-typed output");
        let mut guard = typed.lock().unwrap();
        // Only mark dirty when the value actually changes. This prevents modules
        // that unconditionally call emit! with the same value from preventing the
        // delta-cycle loop from reaching a fixed point.
        if *guard != value {
            *guard = value;
            CURRENT_EMIT_DIRTY.with(|cell| *cell.borrow_mut() = true);
        }
    });
}

/// A future that suspends for exactly one delta cycle, then resumes.
///
/// On the first poll it returns `Pending`, allowing other tasks in the same
/// delta-cycle pass to run.  On the next pass the executor polls it again and
/// it returns `Ready(())`, so the caller continues immediately.
///
/// This is the building block for writing purely combinational modules that
/// re-evaluate every delta cycle instead of every clock edge.  It is also
/// what makes the delta-cycle limitation observable: a module that calls
/// `emit!` unconditionally before `delta_yield().await` will mark the signal
/// dirty on *every* pass, preventing the executor from ever detecting a fixed
/// point and causing it to panic at `MAX_DELTA_CYCLES`.
pub struct DeltaYield {
    yielded: bool,
}

impl Future for DeltaYield {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            Poll::Pending
        }
    }
}

/// Suspend for one delta cycle and resume on the next executor pass.
///
/// See [`DeltaYield`] for full semantics.
pub fn delta_yield() -> DeltaYield {
    DeltaYield { yielded: false }
}

/// A future that blocks during post-edge settle and resolves at the start of
/// the next pre-edge settle.
///
/// Unlike `delta_yield`, which resumes after exactly one delta pass regardless
/// of clock phase, `PreEdgeBarrier` checks the executor's current phase on every
/// poll. It returns `Pending` whenever the executor is in post-edge (even across
/// multiple dirty-driven re-polls), and `Ready` the first time it is polled in
/// pre-edge. This guarantees that the code after the barrier always executes with
/// fresh inputs that were driven by the testbench before the current `tick_clock`
/// call began.
pub struct PreEdgeBarrier;

impl Future for PreEdgeBarrier {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if is_pre_edge() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Return a future that suspends until the executor enters its pre-edge settle
/// phase. Inject this at the end of every sequential module's main loop via
/// `#[hardware(sequential)]` — the macro does this automatically.
pub fn pre_edge_barrier() -> PreEdgeBarrier {
    PreEdgeBarrier
}

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
    ($value:expr) => {{
        $crate::emit_to_current($value);
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
    pub fn dump_vcd(&self, _filename: &str) {
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

#[cfg(test)]
mod tests {
    #[test]
    #[should_panic(expected = "emit!(value) called without a bound function-typed output")]
    fn emit_without_bound_target_panics() {
        crate::emit!(1u8);
    }
}
