use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
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
}

impl HardwareExecutor {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            cycle: 0,
        }
    }

    pub fn spawn<F>(&mut self, future: F) 
    where
        F: Future<Output = ()> + 'static,
    {
        self.tasks.push(Box::pin(future));
    }

    pub fn tick_clock<Domain: ClockDomain>(&mut self, clk: &mut Clock<Domain>) {
        clk.advance();

        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        for task in &mut self.tasks {
            let _ = task.as_mut().poll(&mut context);
        } 

        self.cycle += 1;
    }

    pub fn cycle(&self) -> u64 {
        self.cycle
    }
}

impl Default for HardwareExecutor {
    fn default() -> Self {
        Self::new()
    }
}
