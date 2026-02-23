use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, RawWaker, RawWakerVTable, Waker};

use copper_core::{Clock, ClockDomain, State};

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
    /// Closures that advance each state
    state_commits: Arc<Mutex<Vec<Box<dyn Fn() + Send>>>>,
    cycle: u64,
}

impl HardwareExecutor {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            state_commits: Arc::new(Mutex::new(Vec::new())),
            cycle: 0,
        }
    }
    
    pub fn state_factory(&self) -> StateFactory {
        StateFactory {
            commits: Arc::clone(&self.state_commits),
        }
    }
    
    pub fn spawn(&mut self, task: impl Future<Output = ()> + 'static) {
        self.tasks.push(Box::pin(task));
    }
    
    pub fn tick_clock<Domain: ClockDomain>(&mut self, clk: &mut Clock<Domain>) {
        // 1) Clock edge (wake tasks waiting on tick)
        clk.advance();

        // 2) Commit all registered states (flops update on edge)
        {
            let commits = self.state_commits.lock().unwrap();
            for commit_fn in commits.iter() {
                commit_fn();
            }
        }

        // 3) Poll tasks once (combinational logic based on new state)
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        for task in &mut self.tasks {
            let _ = task.as_mut().poll(&mut cx);
        }

        self.cycle += 1;
    }
    
    pub fn cycle(&self) -> u64 {
        self.cycle
    }
}

#[derive(Clone)]
pub struct StateFactory {
    commits: Arc<Mutex<Vec<Box<dyn Fn() + Send>>>>,
}

impl StateFactory {
    pub fn create<T: Clone + Send + 'static>(&self, initial: T) -> State<T> {
        let state = State::new(initial);
        
        // Create a closure that advances this specific state
        let state_clone = state.clone();
        let commit_fn = move || {
            state_clone.advance_internal();
        };
        
        self.commits.lock().unwrap().push(Box::new(commit_fn));
        
        state
    }
}
