use std::future::Future;
use std::pin::Pin;
use std::collections::HashMap;
use std::task::Context;
use copper_core::port::DirtyHandle;

use copper_core::{Clock, ClockDomain};
use futures::task::noop_waker;

/// A compiled hardware module — the value a `#[hardware]` function returns.
///
/// Only the `#[hardware]` macro can construct one (via the hidden `__new`), so a
/// bare `async fn` produces a plain `Future`, not a `HardwareModule`, and the
/// spawn APIs below refuse it. This makes the attribute **mandatory**: you cannot
/// forget it and accidentally simulate an unprotected module (whose per-read
/// timing guards would be missing). The wrapped future is the module's behavior.
///
/// Spawning a plain `async fn` — forgetting the attribute — does not compile:
///
/// ```compile_fail
/// # use copper_sim::HardwareExecutor;
/// # use copper_core::{Clock, ClockDomain};
/// # use copper_core::port::{wire, Out};
/// # struct C; impl ClockDomain for C {}
/// async fn m(clk: Clock<C>, out: Out<u8, C>) {   // no #[hardware(sequential)]
///     loop { out.write(0); clk.tick().await; }
/// }
/// let mut exec = HardwareExecutor::new();
/// let (o, _obs) = wire::<u8, C>(0);
/// // error[E0308]: expected `HardwareModule<_>`, found future
/// exec.spawn_wired(m(Clock::new(), o), vec![]);
/// ```
pub struct HardwareModule<F> {
    future: F,
}

impl<F> HardwareModule<F> {
    /// Construct a module from its future. **Macro-internal** — called only by
    /// `#[hardware]`-generated code; not meant to be used by hand.
    #[doc(hidden)]
    pub fn __new(future: F) -> Self {
        HardwareModule { future }
    }

    /// Unwrap the future (crate-internal, for the executor to poll).
    pub(crate) fn into_future(self) -> F {
        self.future
    }
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
    /// Number of consecutive delta cycles in which this task's output changed.
    /// Reset to 0 whenever a pass produces no change for this task. Used to detect
    /// combinational loops (a task that never reaches a fixed point).
    consecutive_dirty: usize,
    port_dirties: Vec<DirtyHandle>,
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

    /// Spawn a `#[hardware]` module with no [`DirtyHandle`]s registered — the
    /// executor cannot observe this module's output changes, so it will never
    /// trigger an extra delta cycle on this module's behalf when some other
    /// task's combinational logic depends on its output settling first.
    ///
    /// Named `_untracked` (not just `spawn`) deliberately: the missing
    /// dirty-tracking is a real correctness trap for any design where another
    /// spawned task's combinational settling depends on this module's output,
    /// and the old bare `spawn` name gave no signal at the call site that this
    /// was happening. Safe uses are ones where nothing else in this executor
    /// needs to react to this module's output within a delta-cycle settle pass
    /// — a single self-contained top-level module (e.g. `rv32i_cpu`, whose
    /// output is only read by the testbench after `tick_clock` returns, not by
    /// another spawned task), independent peers with no cross-dependencies
    /// (`counter_by` in `module_composition_hybrid.rs`), or test/probe
    /// scaffolding. If you're wiring a module whose output another task's
    /// combinational logic reads, use [`Self::spawn_wired`] instead.
    pub fn spawn_untracked<F>(&mut self, module: HardwareModule<F>)
    where
        F: Future<Output = ()> + 'static,
    {
        self.tasks.push(TaskEntry {
            future: Box::pin(module.into_future()),
            consecutive_dirty: 0,
            port_dirties: vec![],
        });
    }

    /// Spawn a module, registering its output [`DirtyHandle`]s so the executor
    /// knows when to re-settle.
    ///
    /// `dirties` is the list obtained by calling [`Out::dirty_handle`] on each
    /// output port before handing the `Out` to the module. After each task poll
    /// the executor checks these flags to decide whether another delta cycle is
    /// needed.
    ///
    /// Passing an empty `dirties` vec is valid: the module still runs, but the
    /// executor cannot observe its output changes, so it will never trigger
    /// additional delta cycles on behalf of this task — useful for purely
    /// side-effectful tasks (tracing, logging).
    ///
    /// # Oscillation
    /// If a task is part of a combinational loop, `consecutive_dirty` grows until
    /// it hits `OSCILLATION_THRESHOLD` and the executor panics with a
    /// "combinational loop detected" message.
    pub fn spawn_wired<F>(&mut self, module: HardwareModule<F>, dirties: Vec<DirtyHandle>)
    where
        F: Future<Output = ()> + 'static,
    {
        self.tasks.push(TaskEntry {
            future: Box::pin(module.into_future()),
            consecutive_dirty: 0,
            port_dirties: dirties,
        });
    }

    /// Spawn a child module's future and track the parent-child relationship for
    /// [`Self::module_info`]. Untracked like [`Self::spawn_untracked`] (no
    /// `DirtyHandle`s) — fine for a purely-registered hierarchy where nothing
    /// needs iterative combinational re-settling across children within one
    /// delta-cycle pass (see `module_composition_hybrid.rs`), not for a
    /// hierarchy with combinational dependencies between children.
    pub fn spawn_child<F>(&mut self, child_name: &str, parent_name: &str, module: HardwareModule<F>)
    where
        F: Future<Output = ()> + 'static,
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

        // spawn the child's module as a task in the executor
        self.spawn_untracked(module);
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
    /// value changes — i.e. every signal has reached a fixed point.
    ///
    /// **Convergence** — purely sequential designs (all state behind `clk.tick().await`)
    /// converge in at most two passes.  Acyclic combinational chains take one pass per
    /// level of logic depth.
    ///
    /// **Oscillation detection** — each task tracks `consecutive_dirty`: how many
    /// consecutive delta cycles its output has changed.  For a task in a chain,
    /// this is always 1 (it changes once and then stabilises).  For a task in a
    /// genuine combinational loop the value keeps changing every pass, so
    /// `consecutive_dirty` grows until it hits `OSCILLATION_THRESHOLD`.
    ///
    /// When a task hits the threshold the executor panics with a message
    /// identifying the task index — a genuine combinational loop is a design
    /// error, so it is surfaced eagerly rather than masked.
    pub fn poll_tasks(&mut self) {
        /// A task whose output changes on every delta cycle for this many passes
        /// is diagnosed as part of a combinational loop.
        ///
        /// Rationale: in an acyclic combinational graph a signal is dirty for at
        /// most as many delta cycles as there are independent input chains feeding
        /// into it.  Genuine loops cause the signal to be dirty on every pass
        /// indefinitely.  20 is generous for typical RTL fan-in depths while still
        /// catching loops far earlier than the previous 1000-cycle hard limit.
        const OSCILLATION_THRESHOLD: usize = 20;
        const MAX_DELTA_CYCLES: usize = 1000;

        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        for delta in 0..=MAX_DELTA_CYCLES {
            assert!(
                delta < MAX_DELTA_CYCLES,
                "Delta-cycle limit ({MAX_DELTA_CYCLES}) exceeded — combinational loop not resolved"
            );

            let mut any_dirty = false;
            for (i, task) in self.tasks.iter_mut().enumerate() {
                assert!(
                    task.future.as_mut().poll(&mut context).is_pending(),
                    "hardware module task {i} completed (its future returned Poll::Ready) — a \
                     #[hardware] module's body is a `loop {{ .. }}` that never terminates; a \
                     module future resolving indicates a malformed module (e.g. an early \
                     `return` or a loop that broke), not legal hardware behavior."
                );
                let port_dirty = task.port_dirties.iter().any(|h| h.take());
                if port_dirty {
                    task.consecutive_dirty += 1;
                    any_dirty = true;
                } else {
                    task.consecutive_dirty = 0;
                }
            }

            if !any_dirty {
                // Reset all consecutive counters between settle phases.
                for task in &mut self.tasks {
                    task.consecutive_dirty = 0;
                }
                break;
            }

            // A task still dirty after OSCILLATION_THRESHOLD passes is a
            // combinational loop with no fixed point.
            for (i, task) in self.tasks.iter().enumerate() {
                assert!(
                    task.consecutive_dirty < OSCILLATION_THRESHOLD,
                    "Combinational loop detected: task {i} output changed on {n} consecutive \
                     delta cycles with no fixed point.",
                    i = i,
                    n = task.consecutive_dirty,
                );
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
        crate::synced_read::bump_call_id::<Domain>();

        // Post-edge continuation convention: a `clk.tick()` resolves in the
        // POST-edge settle, so a reaction's post-tick code runs in the SAME
        // `tick_clock`, AFTER the advance. A register clocked at edge N is thus
        // observable in cycle N — the standard synchronous-testbench convention —
        // so the primitive constructs (flip-flop `q<=d`, enabled register,
        // synchronous-read RAM) match hand-written Verilog. Held/registered OUTPUT
        // ports that need one more cycle of latency use an explicit `RegOut`.
        // See design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md.
        //
        // All three phase/resolution/call-id signals are keyed per clock domain
        // (`Domain`), so ticking this clock cannot perturb any other domain's
        // tasks — required for multi-clock designs to be poll-order- and
        // interleave-independent (see design_docs/SYNCHRONOUS_SEMANTICS.md).
        crate::set_poll_phase::<Domain>(crate::PollPhase::PreEdge);
        copper_core::types::set_tick_resolving::<Domain>(false);
        self.poll_tasks();

        clk.advance();

        crate::set_poll_phase::<Domain>(crate::PollPhase::PostEdge);
        copper_core::types::set_tick_resolving::<Domain>(true);
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
    use super::{HardwareExecutor, HardwareModule};
    use copper_core::{Clock, ClockDomain, Logic};

    struct TestClk;
    impl ClockDomain for TestClk {}

    // ── spawn_wired tests ─────────────────────────────────────────────────────

    use copper_core::port::{wire, In, Out};

    // These low-level executor tests drive raw futures directly rather than going
    // through the `#[hardware]` macro (that lives downstream and would inject
    // read-timing machinery unrelated to what they exercise). They wrap in
    // `HardwareModule::__new` — the same thing the macro does — so spawn accepts them.
    fn wired_counter(
        clk: Clock<TestClk>,
        out: Out<u8>,
    ) -> HardwareModule<impl std::future::Future<Output = ()>> {
        HardwareModule::__new(async move {
            let mut value = 0u8;
            loop {
                out.write(value);
                clk.tick().await;
                value = value.wrapping_add(1);
            }
        })
    }

    fn wired_passthrough(
        input: In<u8>,
        out: Out<u8>,
    ) -> HardwareModule<impl std::future::Future<Output = ()>> {
        HardwareModule::__new(async move {
            loop {
                out.write(input.read());
                crate::delta_yield().await;
            }
        })
    }

    #[test]
    fn spawn_wired_sequential_counter_advances_each_cycle() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let (out, in_) = wire::<u8, ()>(0);
        let dirty = out.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), out), vec![dirty]);

        // Post-edge continuation: out.write(value) and the increment both run in
        // the tick's post-edge, so the value written at edge N (after incrementing)
        // is observed in cycle N — the counter reads 1,2,3 (matching `assign q=v`).
        exec.tick_clock(&mut clk);
        assert_eq!(in_.read(), 1);

        exec.tick_clock(&mut clk);
        assert_eq!(in_.read(), 2);

        exec.tick_clock(&mut clk);
        assert_eq!(in_.read(), 3);
    }

    #[test]
    fn spawn_wired_dirty_flag_consumed_after_convergence() {
        // A probe handle (clone of the same dirty flag) should read false after
        // tick_clock, because poll_tasks consumed all dirty flags while settling.
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let (out, _in) = wire::<u8, ()>(0);
        let dirty_for_exec = out.dirty_handle();
        let dirty_probe    = out.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), out), vec![dirty_for_exec]);

        exec.tick_clock(&mut clk);
        assert!(!dirty_probe.take(), "dirty flag must be clear after settling");
    }

    #[test]
    fn spawn_wired_combinational_passthrough_follows_counter() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let (counter_out, counter_in) = wire::<u8, ()>(0);
        let counter_dirty = counter_out.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), counter_out), vec![counter_dirty]);

        let (pass_out, pass_in) = wire::<u8, ()>(0);
        let pass_dirty = pass_out.dirty_handle();
        exec.spawn_wired(wired_passthrough(counter_in, pass_out), vec![pass_dirty]);

        exec.tick_clock(&mut clk);
        assert_eq!(pass_in.read(), 1, "passthrough must reflect counter after one cycle");

        exec.tick_clock(&mut clk);
        assert_eq!(pass_in.read(), 2);
    }

    #[test]
    fn spawn_wired_no_spurious_dirty_when_value_unchanged() {
        // A task that keeps writing the same value must not keep the dirty flag
        // set, which would prevent the executor from finding a fixed point.
        let mut exec = HardwareExecutor::new();

        let (out, in_) = wire::<u8, ()>(42);
        let dirty_probe    = out.dirty_handle();
        let dirty_for_exec = out.dirty_handle();

        exec.spawn_wired(
            HardwareModule::__new(async move {
                loop {
                    out.write(42);
                    crate::delta_yield().await;
                }
            }),
            vec![dirty_for_exec],
        );

        exec.poll_tasks();

        assert_eq!(in_.read(), 42);
        assert!(!dirty_probe.take(), "no dirty after writing identical value");
    }

    #[test]
    fn spawn_wired_multiple_readers_all_see_same_output() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let (out, in_a) = wire::<u8, ()>(0);
        let in_b = in_a.clone();
        let in_c = in_a.clone();
        let dirty = out.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), out), vec![dirty]);

        exec.tick_clock(&mut clk);
        assert_eq!(in_a.read(), 1);
        assert_eq!(in_b.read(), 1);
        assert_eq!(in_c.read(), 1);
    }

    #[test]
    fn spawn_wired_empty_dirties_module_runs_but_changes_are_invisible() {
        // With no dirty handles the executor cannot observe output changes, so it
        // declares a fixed point after the first pass.  The module still ran and
        // wrote to the wire — the value is readable via the In handle.
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let (out, in_) = wire::<u8, ()>(0);
        exec.spawn_wired(wired_counter(clk.clone(), out), vec![]); // no dirty tracking

        exec.tick_clock(&mut clk);
        assert_eq!(in_.read(), 1, "module ran even without dirty tracking");
    }

    #[test]
    fn spawn_wired_two_independent_counters_run_concurrently() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let (out_a, in_a) = wire::<u8, ()>(0);
        let dirty_a = out_a.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), out_a), vec![dirty_a]);

        let (out_b, in_b) = wire::<u8, ()>(0);
        let dirty_b = out_b.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), out_b), vec![dirty_b]);

        exec.tick_clock(&mut clk);
        assert_eq!(in_a.read(), 1);
        assert_eq!(in_b.read(), 1);

        exec.tick_clock(&mut clk);
        assert_eq!(in_a.read(), 2);
        assert_eq!(in_b.read(), 2);
    }
}
