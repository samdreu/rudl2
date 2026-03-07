use std::future::Future;
use std::pin::Pin;
use std::collections::HashMap;
use std::task::{Context, RawWaker, RawWakerVTable, Waker};

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
    tasks: Vec<Pin<Box<dyn Future<Output = ()>>>>,
    // cycle that the exection is on
    cycle: u64,
    // modules included in the execution model
    modules: HashMap<String, ModuleInfo>,
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
        self.tasks.push(Box::pin(wrapped));
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

    /// Get information about a module by name, if it exists. This can be used to inspect the module hierarchy and relationships.
    pub fn module_info(&self, module_name: &str) -> Option<&ModuleInfo> {
        self.modules.get(module_name)
    }

    /// Get a reference to the map of all module infos. This allows for iterating over all modules and their relationships.
    pub fn module_infos(&self) -> &HashMap<String, ModuleInfo> {
        &self.modules
    }

    /// Poll all tasks in the executor. This will drive the execution of all async tasks, allowing them to make progress. 
    /// In a real hardware simulation, this would correspond to allowing all combinational logic to settle and all sequential
    /// logic to update based on the current inputs and clock edge.
    fn poll_tasks(&mut self) {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        for task in &mut self.tasks {
            let _ = task.as_mut().poll(&mut context);
        }
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
