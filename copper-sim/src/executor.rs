use std::future::Future;
use std::pin::Pin;
use std::collections::HashMap;
use std::task::{Context, RawWaker, RawWakerVTable, Waker};

use copper_core::{Clock, ClockDomain};

fn noop_waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VTABLE) }
    fn wake(_: *const ()) {}
    fn wake_by_ref(_: *const ()) {}
    fn drop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

pub struct HardwareExecutor {
    tasks: Vec<Pin<Box<dyn Future<Output = ()>>>>,
    cycle: u64,
    modules: HashMap<String, ModuleInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    pub name: String,
    pub parent: Option<String>,
    pub children: Vec<String>,
}

impl HardwareExecutor {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            cycle: 0,
            modules: HashMap::new(),
        }
    }

    pub fn spawn<F>(&mut self, future: F) 
    where
        F: Future<Output = ()> + 'static,
    {
        self.tasks.push(Box::pin(future));
    }

    pub fn spawn_child<F>(&mut self, child_name: &str, parent_name: &str, future: F)
    where
        F: Future<Output = ()> + 'static,
    {
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

        self.spawn(future);
    }

    pub fn module_info(&self, module_name: &str) -> Option<&ModuleInfo> {
        self.modules.get(module_name)
    }

    pub fn module_infos(&self) -> &HashMap<String, ModuleInfo> {
        &self.modules
    }

    fn poll_tasks(&mut self) {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        for task in &mut self.tasks {
            let _ = task.as_mut().poll(&mut context);
        }
    }

    pub fn tick_clock<Domain: ClockDomain>(&mut self, clk: &mut Clock<Domain>) {
        // Pre-edge settle: allow combinational logic to run
        self.poll_tasks();

        // Advance clock edge
        clk.advance();

        // Post-edge settle: allow sequential logic to update
        self.poll_tasks();

        self.cycle += 1;
    }

    pub fn cycle(&self) -> u64 {
        self.cycle
    }

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
