use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::task::{Context, RawWaker, RawWakerVTable, Waker};
use log::trace;

use copper_core::{Clock, ClockDomain};

/// A no-op waker that can be used to poll futures without needing an actual wake-up mechanism.
/// This is useful in our executor since we will be polling all tasks every cycle, and we don't need to wake them up asynchronously.
fn noop_waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VTABLE) }
    fn wake(_: *const ()) {}
    fn wake_by_ref(_: *const ()) {}
    fn drop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

/// The `HardwareExecutor` manages the execution of hardware modules defined as async tasks. 
/// It tracks the current cycle, the modules in the system, and allows for spawning new tasks and child modules. 
/// It provides a `tick_clock` method to advance the simulation by one clock cycle, 
/// which includes pre-edge and post-edge settling phases to allow combinational and sequential logic to execute properly.
pub struct HardwareExecutor {
    // tasks to be done
    tasks: Vec<TaskEntry>,
    // cycle that the exection is on
    cycle: u64,
    // modules included in the execution model
    modules: HashMap<String, ModuleInfo>,
}

struct TaskEntry {
    future: Pin<Box<dyn Future<Output = ()>>>,
    emit_target: Option<Arc<dyn Any + Send + Sync>>,
}

/// Modules in the execution, with parent-child relationships. This is used for tracking the hierarchy of modules in the system, which can be useful for debugging and visualization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    pub name: String,
    // if there is a parent module, this is the name of the parent. None if top-level
    pub parent: Option<String>,
    // names of child modules
    pub children: Vec<String>,
}

impl HardwareExecutor {
    /// Create a new HardwareExecutor with no tasks, at cycle 0, and an empty module hierarchy.
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            cycle: 0,
            modules: HashMap::new(),
        }
    }

    /// spawn a new task in the executor
    pub fn spawn<F, T>(&mut self, future: F) 
    where
    // F must be a Future that has a 'static lifetime (it doesn't borrow anything)
    // futures are the async equivalent of threads - they represent a sequence of operations that can be paused and resumed
    // they represent a computation that will eventually complete, and can yield control back to the executor when they are waiting for something (like a clock tick)
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        // Wrap the future to discard its output and convert to Future<Output = ()>
        let wrapped = async move {
            let _ = future.await;
        };
        // add the future to the list of tasks to be executed
        // Box it and pin it so it can be polled
        // - Boxing allows us to store different future types in the same vector
        // - Pinning ensures the future's memory location is stable, which is required for async
        self.tasks.push(TaskEntry {
            future: Box::pin(wrapped),
            emit_target: None,
        });
    }

    /// Spawn a function-typed module while automatically allocating a shared output signal.
    ///
    /// This keeps module call sites concise during migration: callers get back the output
    /// handle and do not need to construct the `Arc<Mutex<T>>` manually.
    pub fn spawn_function_typed<T, F>(&mut self, initial_output: T, future: F) -> Arc<Mutex<T>>
    where
        T: PartialEq + Send + 'static,
        F: Future<Output = T> + 'static,
    {
        let output = Arc::new(Mutex::new(initial_output));
        let emit_target: Arc<dyn Any + Send + Sync> = output.clone();

        let wrapped = async move {
            let _ = future.await;
        };
        self.tasks.push(TaskEntry {
            future: Box::pin(wrapped),
            emit_target: Some(emit_target),
        });

        output
    }

    /// spawn a child module's future, and track the parent-child relationship
    pub fn spawn_child<F, T>(&mut self, child_name: &str, parent_name: &str, future: F)
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        // Ensure both parent and child modules are in the module info map
        self.ensure_module(parent_name);
        self.ensure_module(child_name);

        {
            // get parent module info (should exist)
            let parent = self
                .modules
                .get_mut(parent_name)
                .expect("parent module should exist");
            // if child doesn't already exist in parent's children, add it
            if !parent.children.iter().any(|child| child == child_name) {
                parent.children.push(child_name.to_string());
            }
        }

        {
            // get child module info (should exist)
            let child = self
                .modules
                .get_mut(child_name)
                .expect("child module should exist");
            // set child's parent to the given parent name
            child.parent = Some(parent_name.to_string());
        }

        // spawn the child's future as a task in the executor
        self.spawn(future);
    }

    /// Spawn a function-typed child module, track hierarchy, and bind an implicit emit target.
    pub fn spawn_child_function_typed<T, F>(
        &mut self,
        child_name: &str,
        parent_name: &str,
        initial_output: T,
        future: F,
    ) -> Arc<Mutex<T>>
    where
        T: PartialEq + Send + Sync + 'static,
        F: Future<Output = T> + 'static,
    {
        // Ensure both parent and child modules are in the module info map
        self.ensure_module(parent_name);
        self.ensure_module(child_name);

        {
            let parent = self
                .modules
                .get_mut(parent_name)
                .expect("parent module should exist");
            if !parent.children.iter().any(|child| child == child_name) {
                parent.children.push(child_name.to_string());
            }
        }

        {
            let child = self
                .modules
                .get_mut(child_name)
                .expect("child module should exist");
            child.parent = Some(parent_name.to_string());
        }

        let output = Arc::new(Mutex::new(initial_output));
        let emit_target: Arc<dyn Any + Send + Sync> = output.clone();
        let wrapped = async move {
            let _ = future.await;
        };
        self.tasks.push(TaskEntry {
            future: Box::pin(wrapped),
            emit_target: Some(emit_target),
        });

        output
    }

    /// Get information about a module by name, if it exists. This can be used to inspect the module hierarchy and relationships.
    pub fn module_info(&self, module_name: &str) -> Option<&ModuleInfo> {
        self.modules.get(module_name)
    }

    /// Get a reference to the map of all module infos. This allows for iterating over all modules and their relationships.
    pub fn module_infos(&self) -> &HashMap<String, ModuleInfo> {
        &self.modules
    }

    /// Poll all tasks in a delta-cycle loop until no signal changes.
    ///
    /// One call = one simulation phase (pre-edge or post-edge settle).  Within that
    /// phase the executor repeatedly polls every task until a full pass produces no
    /// new `emit!` calls — i.e. every signal has reached a fixed point.
    ///
    /// For purely sequential designs (all state behind `clk.tick().await`) this
    /// converges in at most two passes: tasks emit on the first pass and sleep on
    /// the second.  For acyclic combinational chains the loop propagates changes
    /// one level per pass.  A genuine combinational loop (A drives B drives A with
    /// no register) will never converge and panics once `MAX_DELTA_CYCLES` is hit.
    pub fn poll_tasks(&mut self) {
        const MAX_DELTA_CYCLES: usize = 1000;

        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        for delta in 0..=MAX_DELTA_CYCLES {
            assert!(
                delta < MAX_DELTA_CYCLES,
                "Delta-cycle limit ({MAX_DELTA_CYCLES}) exceeded — \
                 possible combinational loop in the design"
            );

            let mut any_dirty = false;
            for task in &mut self.tasks {
                // push_emit_target resets the dirty flag before the poll
                let _emit_guard = crate::push_emit_target(task.emit_target.clone());
                let _ = task.future.as_mut().poll(&mut context);
                // take_emit_dirty reads and resets the flag set by emit!
                if crate::take_emit_dirty() {
                    any_dirty = true;
                }
            }

            if !any_dirty {
                break; // fixed point reached
            }
        }
    }
    /// Advance the clock edge only (no polling)
    pub fn advance<Domain: ClockDomain>(&mut self, clk: &mut Clock<Domain>) {
        clk.advance();
        self.cycle += 1;
    }

    /// Advance the simulation by one clock cycle. 
    /// This includes a pre-edge settle phase where all tasks are polled to allow combinational logic to run, 
    /// then the clock is advanced, and then a post-edge settle phase where tasks are polled again to allow sequential logic to update based on the new clock edge.
    pub fn tick_clock<Domain: ClockDomain>(&mut self, clk: &mut Clock<Domain>) {
        // Pre-edge settle: allow combinational logic to run
        self.poll_tasks();

        // Advance clock edge
        clk.advance();

        // Post-edge settle: allow sequential logic to update
        self.poll_tasks();

        self.cycle += 1;
    }

    /// Get the current cycle number. This can be used for tracking the progress of the simulation and for debugging purposes.
    pub fn cycle(&self) -> u64 {
        self.cycle
    }

    /// Ensure a module exists in the module info map, creating it if necessary
    fn ensure_module(&mut self, module_name: &str) {
        self.modules
            .entry(module_name.to_string())
            .or_insert_with(|| ModuleInfo {
                name: module_name.to_string(),
                parent: None,
                children: Vec::new(),
            });
    }
}

impl Default for HardwareExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::HardwareExecutor;
    use copper_core::{Clock, ClockDomain, Logic};

    struct TestClk;
    impl ClockDomain for TestClk {}

    async fn counter_u8(clk: Clock<TestClk>) -> u8 {
        let mut value = 0u8;
        loop {
            crate::emit!(value);
            clk.tick().await;
            value = value.wrapping_add(1);
        }
    }

    async fn counter_tuple(clk: Clock<TestClk>) -> (u8, Logic) {
        let mut value = 0u8;
        loop {
            let logic = if value & 1 == 1 { Logic::One } else { Logic::Zero };
            crate::emit!((value, logic));
            clk.tick().await;
            value = value.wrapping_add(1);
        }
    }

    #[test]
    fn spawn_function_typed_emits_values() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let out = exec.spawn_function_typed(0u8, counter_u8(clk.clone()));

        exec.tick_clock(&mut clk);
        assert_eq!(*out.lock().unwrap(), 1u8);

        exec.tick_clock(&mut clk);
        assert_eq!(*out.lock().unwrap(), 2u8);
    }

    #[test]
    fn spawn_function_typed_supports_tuple_outputs() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let out = exec.spawn_function_typed((0u8, Logic::Zero), counter_tuple(clk.clone()));

        exec.tick_clock(&mut clk);
        assert_eq!(*out.lock().unwrap(), (1u8, Logic::One));

        exec.tick_clock(&mut clk);
        assert_eq!(*out.lock().unwrap(), (2u8, Logic::Zero));
    }

    #[test]
    fn spawn_child_function_typed_tracks_hierarchy_and_emits() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let out = exec.spawn_child_function_typed(
            "child_a",
            "parent_a",
            0u8,
            counter_u8(clk.clone()),
        );

        exec.tick_clock(&mut clk);
        assert_eq!(*out.lock().unwrap(), 1u8);

        let parent = exec.module_info("parent_a").expect("missing parent module info");
        assert!(parent.children.iter().any(|c| c == "child_a"));

        let child = exec.module_info("child_a").expect("missing child module info");
        assert_eq!(child.parent.as_deref(), Some("parent_a"));
    }
}
