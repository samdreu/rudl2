use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use copper_core::{Clock, ClockDomain};

// what is this used for?
// explain every part of this function - why do we need a noop waker? what is the VTABLE doing? why are the functions empty?
fn noop_waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VTABLE) }
    fn wake(_: *const ()) {}
    fn wake_by_ref(_: *const ()) {}
    fn drop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}


pub struct HardwareExecutor {
    tasks: Vec<Pin<Box<dyn Future<Output = ()>>>>, // why is this the appropriate type here?
    cycle: u64 // the global cycle count - independent of the tasks cycles
}

impl HardwareExecutor {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            cycle: 0,
        }
    }

    pub fn spawn(&mut self, task: impl Future<Output = ()> + 'static) {
        self.tasks.push(Box::pin(task)); // what is the Box::pin(task) doing here?
    }

    pub fn tick(&mut self) {
        // poll all tasks once, advance clocks, commit states
        self.cycle += 1;
    }

    pub fn cycle(&self) -> u64 {
        self.cycle
    }

    pub fn tick_clock<Domain: ClockDomain>(&mut self, clk: &mut Clock<Domain>) {
        clk.advance();

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        for task in &mut self.tasks {
            let _ = task.as_mut().poll(&mut cx);
        }

        self.cycle += 1;
    }

    /// Poll all tasks once (no clock advance).
    pub fn poll_tasks(&mut self) {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        for task in &mut self.tasks {
            let _ = task.as_mut().poll(&mut cx);
        }
    }
}
