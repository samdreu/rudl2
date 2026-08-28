use copper_core::ClockDomain;
use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

pub mod verification;
pub use verification::{
    is_missing_verilator, verify_with_verilator, verilator_status, CycleData, SimulationTrace,
    VERILATOR_NOT_INSTALLED,
};

pub mod testing;
pub use testing::{HardwareTest, TestResult, make_cycle};

pub mod executor;

pub use executor::{HardwareExecutor, HardwareModule, ModuleInfo, PollOrder, SchedulerMode};

/// Which phase of the clock cycle `tick_clock` is currently executing.
/// Set by the executor before each `poll_tasks()` call so that phase-aware futures
/// (`PreEdgeBarrier`) can determine whether to block or proceed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollPhase {
    PreEdge,
    PostEdge,
}

thread_local! {
    /// Current clock phase set by `tick_clock` before each settle pass, keyed per
    /// clock domain (by `TypeId`) so that advancing one domain's clock cannot
    /// perturb another domain's phase-gated futures. A domain with no entry yet
    /// defaults to `PreEdge` (see `is_pre_edge`).
    static POLL_PHASE: RefCell<HashMap<TypeId, PollPhase>> = RefCell::new(HashMap::new());
}

pub(crate) fn set_poll_phase<Domain: ClockDomain>(phase: PollPhase) {
    POLL_PHASE.with(|cell| {
        cell.borrow_mut().insert(TypeId::of::<Domain>(), phase);
    });
}

fn is_pre_edge<Domain: ClockDomain>() -> bool {
    POLL_PHASE.with(|cell| {
        cell.borrow().get(&TypeId::of::<Domain>()).copied().unwrap_or(PollPhase::PreEdge)
            == PollPhase::PreEdge
    })
}

/// A future that suspends for exactly one delta cycle, then resumes.
///
/// On the first poll it returns `Pending`, allowing other tasks in the same
/// delta-cycle pass to run.  On the next pass the executor polls it again and
/// it returns `Ready(())`, so the caller continues immediately.
///
/// This is the building block for writing purely combinational modules that
/// re-evaluate every delta cycle instead of every clock edge.  It is also
/// what makes the delta-cycle limitation observable: a module that drives an
/// output to a new value unconditionally before `delta_yield().await` will mark
/// the signal dirty on *every* pass, preventing the executor from ever detecting
/// a fixed point and causing it to panic at the oscillation threshold.
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
/// the next pre-edge settle, for its own clock domain `Domain`.
///
/// Unlike `delta_yield`, which resumes after exactly one delta pass regardless
/// of clock phase, `PreEdgeBarrier` checks `Domain`'s current phase on every
/// poll. It returns `Pending` whenever `Domain` is in post-edge (even across
/// multiple dirty-driven re-polls), and `Ready` the first time it is polled in
/// pre-edge. This guarantees that the code after the barrier always executes with
/// fresh inputs that were driven by the testbench before the current `tick_clock`
/// call began. Scoped per domain so a barrier for one clock is never woken by
/// another clock's phase transitions.
pub struct PreEdgeBarrier<Domain: ClockDomain> {
    _domain: PhantomData<Domain>,
}

impl<Domain: ClockDomain> Future for PreEdgeBarrier<Domain> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if is_pre_edge::<Domain>() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Return a future that suspends until `Domain`'s clock enters its pre-edge
/// settle phase. Inject this at the end of every sequential module's main loop
/// via `#[hardware(sequential)]` — the macro does this automatically.
pub fn pre_edge_barrier<Domain: ClockDomain>() -> PreEdgeBarrier<Domain> {
    PreEdgeBarrier { _domain: PhantomData }
}

/// A helper macro to spawn a child module's future and track the parent-child
/// relationship in the executor.
///
/// The trailing `reads` argument is the child's input wire-ids (`vec![
/// port.wire_id(), .. ]`), recorded for the item-6 dependency graph. Phase 1
/// records only — it has no effect on the current scheduler.
#[macro_export]
macro_rules! spawn_child {
    ($exec:expr, $parent:expr, $module_future:expr, $reads:expr) => {{
        $exec.spawn_child(stringify!($module_future), $parent, $module_future, $reads)
    }};
    ($exec:expr, $parent:expr, $child_name:expr, $module_future:expr, $reads:expr) => {{
        $exec.spawn_child($child_name, $parent, $module_future, $reads)
    }};
}

