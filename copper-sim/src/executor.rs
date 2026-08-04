use std::future::Future;
use std::pin::Pin;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::task::Context;
use copper_core::port::{DirtyHandle, WireId, WireKind};

use copper_core::{Clock, ClockDomain};
use futures::task::noop_waker;

/// A task whose output changes on every delta cycle for this many consecutive
/// passes is diagnosed as part of a combinational loop.
///
/// Rationale: in an acyclic combinational graph a signal is dirty for at most as
/// many delta cycles as there are independent input chains feeding into it.
/// Genuine loops cause the signal to be dirty on every pass indefinitely. 20 is
/// generous for typical RTL fan-in depths while still catching loops far earlier
/// than the `MAX_DELTA_CYCLES` hard limit. Shared by the fixpoint settle and the
/// per-SCC iteration of the levelized settle (item 6).
const OSCILLATION_THRESHOLD: usize = 20;
/// Hard upper bound on delta cycles in a single settle (fixpoint loop or an SCC
/// iteration) before the executor gives up on convergence.
const MAX_DELTA_CYCLES: usize = 1000;

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
/// exec.spawn_wired(m(Clock::new(), o), vec![], vec![]);
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
    // order in which poll_tasks visits tasks within each delta cycle
    poll_order: PollOrder,
    // which settling algorithm poll_tasks uses (item 6)
    scheduler: SchedulerMode,
    // cached levelized settle plan (item 6): the tasks' strongly-connected
    // components over the combinational-dependency graph, in a topological order
    // (each producer SCC before its consumers). A length-1 component is settled in
    // a single poll; a genuine combinational cycle is one multi-task component,
    // iterated to a fixpoint within itself. Lazily built; `graph_dirty` forces a
    // rebuild after a spawn.
    levelized_plan: Vec<Vec<usize>>,
    graph_dirty: bool,
}

/// Which settling algorithm [`HardwareExecutor::poll_tasks`] uses within a phase
/// (item 6 — levelized dependency-graph scheduling).
///
/// [`SchedulerMode::Fixpoint`] is the default and the production behavior: poll
/// every task repeatedly until a full pass produces no change (the delta-cycle
/// loop). [`SchedulerMode::Levelized`] instead polls each task **once** in a
/// topological order derived from the inter-module combinational dependency graph
/// — a consumer is always polled after every producer whose *plain-`Out`* output
/// it reads, so a single pass settles an acyclic combinational graph. It is
/// **provably equivalent** to the fixpoint settle on any well-formed design and is
/// validated differentially against it corpus-wide (`tests/levelized_differential.rs`).
///
/// The fixpoint scheduler stays in the tree permanently as the differential oracle
/// even after levelized becomes the default (like the [`PollOrder`] knob). A
/// genuine combinational cycle becomes one multi-task strongly-connected component
/// that is iterated to a fixpoint *within itself* while the acyclic remainder is
/// still single-passed (item-6 phase 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SchedulerMode {
    /// Iterate-to-fixpoint delta-cycle loop — retained permanently as the
    /// differential oracle and a fallback, no longer the production default.
    Fixpoint,
    /// SCC-condensed levelized settle over the combinational dependency graph
    /// (item 6) — **the production default** since phase 4. Poll-order independence
    /// is structural under it: it settles in a canonical topological order.
    #[default]
    Levelized,
}

impl SchedulerMode {
    /// The scheduler a fresh [`HardwareExecutor`] uses by default.
    ///
    /// Honors a `COPPER_SCHEDULER` environment override (`fixpoint` | `levelized`,
    /// case-insensitive) so the entire corpus can be run under either scheduler
    /// with no call-site edits — used to validate the levelized scheduler against
    /// the fixpoint oracle corpus-wide, and as a permanent escape hatch. An unset
    /// or unrecognized value uses the compiled default below. An explicit
    /// [`HardwareExecutor::with_scheduler_mode`] / [`HardwareExecutor::set_scheduler_mode`]
    /// always overrides this.
    fn env_default() -> SchedulerMode {
        match std::env::var("COPPER_SCHEDULER") {
            Ok(v) if v.eq_ignore_ascii_case("fixpoint") => SchedulerMode::Fixpoint,
            Ok(v) if v.eq_ignore_ascii_case("levelized") => SchedulerMode::Levelized,
            // Compiled default: Levelized is the production scheduler since item-6
            // phase 4 (validated corpus-wide against the fixpoint oracle).
            _ => SchedulerMode::Levelized,
        }
    }
}

/// Tarjan's strongly-connected-components algorithm over a directed graph given as
/// an adjacency list, returning each node's component id (`comp_id[node]`).
///
/// Iterative (an explicit DFS stack) so a deep dependency chain cannot overflow
/// the call stack. Component ids are assigned as SCCs are finalized, which is
/// reverse-topological order of the condensation; the caller re-derives an explicit
/// topological order, so the exact ids here are not relied upon. Node iteration is
/// index-ordered, so the result is deterministic.
fn tarjan_scc(n: usize, adj: &[Vec<usize>]) -> Vec<usize> {
    const UNVISITED: usize = usize::MAX;
    let mut index = vec![UNVISITED; n]; // DFS discovery order
    let mut low = vec![0usize; n]; // lowlink
    let mut on_stack = vec![false; n];
    let mut comp_id = vec![UNVISITED; n];
    let mut scc_stack: Vec<usize> = Vec::new();
    let mut next_index = 0usize;
    let mut next_comp = 0usize;

    for root in 0..n {
        if index[root] != UNVISITED {
            continue;
        }
        // Explicit DFS: each frame is (node, next-neighbor-cursor).
        let mut call: Vec<(usize, usize)> = Vec::new();
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        scc_stack.push(root);
        on_stack[root] = true;
        call.push((root, 0));

        while let Some(&(v, cursor)) = call.last() {
            if cursor < adj[v].len() {
                call.last_mut().unwrap().1 += 1;
                let w = adj[v][cursor];
                if index[w] == UNVISITED {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    scc_stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                // All of v's edges explored: if v is an SCC root, pop the SCC.
                if low[v] == index[v] {
                    loop {
                        let x = scc_stack.pop().unwrap();
                        on_stack[x] = false;
                        comp_id[x] = next_comp;
                        if x == v {
                            break;
                        }
                    }
                    next_comp += 1;
                }
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    low[parent] = low[parent].min(low[v]);
                }
            }
        }
    }
    comp_id
}

/// Order in which the **fixpoint** settle ([`SchedulerMode::Fixpoint`]) visits
/// tasks within a delta cycle.
///
/// The whole project rests on the invariant (CLAUDE.md,
/// `design_docs/SYNCHRONOUS_SEMANTICS.md`) that **a well-formed design simulates
/// identically under any poll order** — the sim must not depend on Rust async
/// poll order. Under the production [`SchedulerMode::Levelized`] scheduler this is
/// now *structural* (it settles in a canonical topological order and ignores this
/// knob entirely); the knob applies only to the fixpoint oracle, where the fuzzer
/// (`tests/poll_order_fuzz.rs`) still drives adversarial orders to guard the
/// oracle's poll-order independence (gate G3). Changing the order must never change
/// the settled values of a well-formed design; it can only change how many
/// delta-cycle passes the fixed-point loop takes to get there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PollOrder {
    /// Spawn order (the default; production behavior).
    #[default]
    Insertion,
    /// Reverse spawn order — a fixed adversarial permutation, cheap to reason about.
    Reversed,
    /// Reshuffle the visit order every delta cycle with a deterministic PRNG
    /// seeded by this value. The strongest guard: it perturbs order *within* the
    /// settle loop, not just across runs, and is reproducible for a given seed.
    Seeded(u64),
}

/// SplitMix64 — a tiny, dependency-free deterministic PRNG for [`PollOrder::Seeded`]
/// shuffling. Not cryptographic; just needs to be reproducible and well-spread.
#[inline]
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

struct TaskEntry {
    future: Pin<Box<dyn Future<Output = ()>>>,
    /// Number of consecutive delta cycles in which this task's output changed.
    /// Reset to 0 whenever a pass produces no change for this task. Used to detect
    /// combinational loops (a task that never reaches a fixed point).
    consecutive_dirty: usize,
    port_dirties: Vec<DirtyHandle>,
    /// The wires this task *reads* (its dependency-graph in-edges). Recorded at
    /// spawn time so the executor can build a producer→consumer graph: a task's
    /// out-edges are the combinational wires its `port_dirties` drive (each
    /// [`DirtyHandle`] carries its [`WireId`] and [`WireKind`]), and its in-edges
    /// are these reads. Consumed by [`Self::compute_topo_order`] under
    /// [`SchedulerMode::Levelized`]; the fixpoint scheduler ignores it.
    reads: Vec<WireId>,
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
            poll_order: PollOrder::Insertion,
            scheduler: SchedulerMode::env_default(),
            levelized_plan: Vec::new(),
            graph_dirty: true,
        }
    }

    /// Select the settling algorithm (item 6). The default is
    /// [`SchedulerMode::Levelized`] (overridable via `COPPER_SCHEDULER`); the
    /// differential harness and the poll-order fuzzer use this to pin
    /// [`SchedulerMode::Fixpoint`], the permanent oracle. See [`SchedulerMode`].
    pub fn set_scheduler_mode(&mut self, mode: SchedulerMode) {
        self.scheduler = mode;
    }

    /// Builder form of [`Self::set_scheduler_mode`].
    pub fn with_scheduler_mode(mut self, mode: SchedulerMode) -> Self {
        self.scheduler = mode;
        self
    }

    /// Set the order in which the fixpoint settle visits tasks. Has **no effect**
    /// under the production [`SchedulerMode::Levelized`] scheduler (it computes its
    /// own canonical order); it exists for the poll-order fuzzer, which pins
    /// [`SchedulerMode::Fixpoint`] to guard the oracle (gate G3). See [`PollOrder`].
    pub fn set_poll_order(&mut self, order: PollOrder) {
        self.poll_order = order;
    }

    /// Builder form of [`Self::set_poll_order`].
    pub fn with_poll_order(mut self, order: PollOrder) -> Self {
        self.poll_order = order;
        self
    }

    /// The task-visit order for delta cycle `delta` under the current
    /// [`PollOrder`]. Returned as an owned index permutation so the caller can
    /// then borrow `self.tasks` mutably while iterating it.
    fn visit_order(&self, delta: usize) -> Vec<usize> {
        let n = self.tasks.len();
        match self.poll_order {
            PollOrder::Insertion => (0..n).collect(),
            PollOrder::Reversed => (0..n).rev().collect(),
            PollOrder::Seeded(seed) => {
                let mut idx: Vec<usize> = (0..n).collect();
                // Re-seed per delta cycle so the order varies across the settle
                // loop, not just across runs.
                let mut state =
                    splitmix64(seed ^ (delta as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                for i in (1..n).rev() {
                    state = splitmix64(state);
                    let j = (state % (i as u64 + 1)) as usize;
                    idx.swap(i, j);
                }
                idx
            }
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
    ///
    /// `reads` is the list of input wire-ids the module reads
    /// ([`In::wire_id`](copper_core::port::In::wire_id) on each input port),
    /// recorded for the item-6 dependency graph. It has no effect on the current
    /// fixpoint scheduler (phase 1 records only); pass `vec![]` for a module with
    /// no wire inputs.
    pub fn spawn_untracked<F>(&mut self, module: HardwareModule<F>, reads: Vec<WireId>)
    where
        F: Future<Output = ()> + 'static,
    {
        self.tasks.push(TaskEntry {
            future: Box::pin(module.into_future()),
            consecutive_dirty: 0,
            port_dirties: vec![],
            reads,
        });
        self.graph_dirty = true;
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
    ///
    /// `reads` is the list of input wire-ids the module reads
    /// ([`In::wire_id`](copper_core::port::In::wire_id) on each input port),
    /// recorded for the item-6 dependency graph (producer→consumer edges are built
    /// by matching a task's `reads` against other tasks' output wire-ids, which the
    /// `dirties` carry). Phase 1 records only — it has no effect on the current
    /// fixpoint scheduler; pass `vec![]` for a module with no wire inputs.
    pub fn spawn_wired<F>(
        &mut self,
        module: HardwareModule<F>,
        dirties: Vec<DirtyHandle>,
        reads: Vec<WireId>,
    ) where
        F: Future<Output = ()> + 'static,
    {
        self.tasks.push(TaskEntry {
            future: Box::pin(module.into_future()),
            consecutive_dirty: 0,
            port_dirties: dirties,
            reads,
        });
        self.graph_dirty = true;
    }

    /// Spawn a child module's future and track the parent-child relationship for
    /// [`Self::module_info`]. Untracked like [`Self::spawn_untracked`] (no
    /// `DirtyHandle`s) — fine for a purely-registered hierarchy where nothing
    /// needs iterative combinational re-settling across children within one
    /// delta-cycle pass (see `module_composition_hybrid.rs`), not for a
    /// hierarchy with combinational dependencies between children.
    ///
    /// `reads` is the child's input wire-ids (recorded for the item-6 dependency
    /// graph; phase 1 records only, no scheduling effect).
    pub fn spawn_child<F>(
        &mut self,
        child_name: &str,
        parent_name: &str,
        module: HardwareModule<F>,
        reads: Vec<WireId>,
    ) where
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
        self.spawn_untracked(module, reads);
    }

    /// Get information about a module by name, if it exists. This can be used to inspect the module hierarchy and relationships.
    pub fn module_info(&self, module_name: &str) -> Option<&ModuleInfo> {
        self.modules.get(module_name)
    }

    /// Get a reference to the map of all module infos. This allows for iterating over all modules and their relationships.
    pub fn module_infos(&self) -> &HashMap<String, ModuleInfo> {
        &self.modules
    }

    /// The **combinational loops** in the currently-spawned design: each inner vec
    /// is the set of task indices forming a strongly-connected component of size ≥ 2
    /// in the combinational dependency graph (item 6). Empty for a well-formed,
    /// acyclic design.
    ///
    /// This is *static* detection — it inspects the wiring graph without running
    /// the simulation. A multi-task component is a combinational cycle: mutual
    /// plain-`Out` feedback with no register (`RegOut`), memory, or synchronizer to
    /// break it (those commit at the clock edge and so induce no combinational
    /// edge). **Detection is not rejection:** a *convergent* loop (e.g. a
    /// set-dominant latch) is legal and simulated by iterating the component to a
    /// fixpoint; only a *non-convergent* loop panics, and only when the settle
    /// actually fails to converge (see [`Self::poll_tasks`]/`iterate_scc`). Use this
    /// to surface a design's combinational loops for inspection or a lint.
    pub fn comb_cycles(&self) -> Vec<Vec<usize>> {
        self.compute_scc_plan()
            .into_iter()
            .filter(|component| component.len() > 1)
            .collect()
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
    ///
    /// Dispatches on [`SchedulerMode`]: the default [`SchedulerMode::Levelized`]
    /// runs the SCC-condensed levelized settle (item 6); [`SchedulerMode::Fixpoint`]
    /// (the differential oracle) runs the delta-cycle loop described above.
    pub fn poll_tasks(&mut self) {
        match self.scheduler {
            SchedulerMode::Fixpoint => self.poll_tasks_fixpoint(),
            SchedulerMode::Levelized => {
                self.ensure_graph();
                let plan = std::mem::take(&mut self.levelized_plan);
                self.poll_tasks_levelized(&plan);
                self.levelized_plan = plan;
            }
        }
    }

    /// The levelized settle: walk the combinational-dependency SCCs in topological
    /// order (item 6, phase 3).
    ///
    /// Each component's combinational inputs are final by the time it is reached
    /// (every producer SCC precedes it in `plan`), so:
    /// - a **singleton** component (the overwhelmingly common case — well-formed
    ///   designs have acyclic combinational graphs) is settled in a **single poll**;
    /// - a **multi-task** component is a genuine combinational cycle (mutual
    ///   plain-`Out` feedback) and is iterated to a fixpoint *within itself*
    ///   ([`Self::iterate_scc`]) — the acyclic remainder is unaffected.
    ///
    /// This reaches the same global fixed point as [`Self::poll_tasks_fixpoint`]
    /// (a topological sweep with per-SCC fixpoints equals the whole-graph fixpoint),
    /// which the differential harness verifies corpus-wide.
    fn poll_tasks_levelized(&mut self, plan: &[Vec<usize>]) {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        for component in plan {
            if component.len() == 1 {
                self.poll_task_once(component[0], &mut context);
            } else {
                self.iterate_scc(component, &mut context);
            }
        }
    }

    /// Poll one task exactly once and consume its dirty flags. Used for singleton
    /// (acyclic) components, whose inputs are already final. A single poll cannot
    /// oscillate, so there is no threshold check.
    fn poll_task_once(&mut self, i: usize, context: &mut Context<'_>) {
        let task = &mut self.tasks[i];
        assert!(
            task.future.as_mut().poll(context).is_pending(),
            "hardware module task {i} completed (its future returned Poll::Ready) — a \
             #[hardware] module's body is a `loop {{ .. }}` that never terminates; a \
             module future resolving indicates a malformed module (e.g. an early \
             `return` or a loop that broke), not legal hardware behavior."
        );
        for h in &task.port_dirties {
            h.take();
        }
    }

    /// Iterate a genuine combinational cycle (a multi-task SCC) to a fixpoint,
    /// polling only the SCC's own tasks. Bounded and oscillation-checked, but scoped
    /// to the cycle — a *convergent* combinational loop (e.g. a set-dominant latch)
    /// settles here, reproducing the fixpoint scheduler; a *non-convergent* one
    /// trips [`OSCILLATION_THRESHOLD`] and is reported as a combinational loop.
    ///
    /// **Item 6, phase 5 — static comb-loop detection.** A multi-task SCC is a
    /// combinational cycle *identified structurally* (from the dependency graph),
    /// so when the settle fails to converge the panic names the **whole cycle** and
    /// what to do about it — replacing the old vague single-task oscillation panic.
    /// Detection is not rejection: a convergent cycle is legal and simulated (per
    /// the plan, an error is emitted *only* for a loop the model cannot resolve, so
    /// we never reject a design the fixpoint oracle accepts).
    fn iterate_scc(&mut self, scc: &[usize], context: &mut Context<'_>) {
        for delta in 0..=MAX_DELTA_CYCLES {
            assert!(
                delta < MAX_DELTA_CYCLES,
                "Delta-cycle limit ({MAX_DELTA_CYCLES}) exceeded — combinational loop among \
                 tasks {scc:?} not resolved"
            );

            let mut any_dirty = false;
            for &i in scc {
                let task = &mut self.tasks[i];
                assert!(
                    task.future.as_mut().poll(context).is_pending(),
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
                for &i in scc {
                    self.tasks[i].consecutive_dirty = 0;
                }
                break;
            }

            // Still dirty after OSCILLATION_THRESHOLD passes → this SCC is a
            // combinational loop with no fixed point. Report the whole cycle
            // structurally (the statically-detected SCC), not just one task.
            if scc.iter().any(|&i| self.tasks[i].consecutive_dirty >= OSCILLATION_THRESHOLD) {
                panic!(
                    "Combinational loop detected: tasks {scc:?} form a combinational cycle \
                     (each drives a plain `Out` that the next reads, with no register \
                     (`RegOut`), memory, or synchronizer to break it) and it does not settle \
                     to a fixed point after {OSCILLATION_THRESHOLD} delta cycles. Break the \
                     cycle with a registered output (`RegOut`) or a synchronizer."
                );
            }
        }
    }

    /// (Re)build the cached levelized settle plan if a task has been spawned since
    /// the last build. Lazy so a run of spawns costs one build, not one per spawn.
    fn ensure_graph(&mut self) {
        if !self.graph_dirty {
            return;
        }
        self.levelized_plan = self.compute_scc_plan();
        self.graph_dirty = false;
    }

    /// Build the inter-module **combinational** dependency graph as an adjacency
    /// list `producer → consumer`.
    ///
    /// An edge exists where the producer drives a wire via a plain `Out` (a
    /// [`WireKind::Comb`] output, learned from its `port_dirties`) and the consumer
    /// reads that wire (its recorded `reads`). `RegOut` outputs are **excluded**: a
    /// registered output commits at the clock edge, so it is constant during a
    /// settle — treating it as a sink would forge false cycles out of legal
    /// sequential feedback (see `LEVELIZED_SCHEDULING_SCOPE.md`). Self-edges and
    /// duplicate edges are dropped.
    fn comb_dependency_graph(&self) -> Vec<Vec<usize>> {
        let n = self.tasks.len();

        // Each combinational wire's single producer (`Out` is non-Clone → one
        // writer per wire, so this map is a function).
        let mut comb_producer: HashMap<WireId, usize> = HashMap::new();
        for (i, task) in self.tasks.iter().enumerate() {
            for h in &task.port_dirties {
                if h.wire_kind() == WireKind::Comb {
                    comb_producer.insert(h.wire_id(), i);
                }
            }
        }

        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (c, task) in self.tasks.iter().enumerate() {
            // Dedupe so a consumer reading several wires from one producer yields a
            // single edge.
            let mut seen: HashSet<usize> = HashSet::new();
            for w in &task.reads {
                if let Some(&p) = comb_producer.get(w) {
                    // Skip self-edges: an intra-module combinational self-read is
                    // unexpressible in Copper.
                    if p != c && seen.insert(p) {
                        adj[p].push(c);
                    }
                }
            }
        }
        adj
    }

    /// Build the levelized settle plan: the tasks' strongly-connected components
    /// over the combinational graph, in a topological order (every producer SCC
    /// before its consumers).
    ///
    /// Well-formed designs have an **acyclic** combinational graph, so every
    /// component is a singleton and this is just a topological sort. A genuine
    /// combinational cycle (mutual plain-`Out` feedback) collapses into one
    /// multi-task component, which the settle iterates to a fixpoint in isolation —
    /// the acyclic remainder still settles in a single pass. This condensation is a
    /// DAG by construction, so a valid order always exists (no cyclic "give up"
    /// case).
    fn compute_scc_plan(&self) -> Vec<Vec<usize>> {
        let n = self.tasks.len();
        let adj = self.comb_dependency_graph();
        let comp_id = tarjan_scc(n, &adj);
        let num_comps = comp_id.iter().copied().max().map_or(0, |m| m + 1);

        // Group tasks by component; sort members ascending so a component's minimum
        // task index is `members[c][0]` (used for a deterministic tie-break).
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); num_comps];
        for (i, &c) in comp_id.iter().enumerate() {
            members[c].push(i);
        }
        for m in &mut members {
            m.sort_unstable();
        }

        // Condensation edges + in-degrees (dedup parallel edges between components).
        let mut cadj: Vec<HashSet<usize>> = vec![HashSet::new(); num_comps];
        let mut cindeg: Vec<usize> = vec![0; num_comps];
        for (u, outs) in adj.iter().enumerate() {
            for &v in outs {
                let (cu, cv) = (comp_id[u], comp_id[v]);
                if cu != cv && cadj[cu].insert(cv) {
                    cindeg[cv] += 1;
                }
            }
        }

        // Kahn over the condensation, tie-broken by each component's minimum task
        // index (a min-heap) so the order is deterministic and, among independent
        // components, closest to spawn order — the canonical order that makes
        // poll-order independence *structural*.
        let mut ready: BinaryHeap<Reverse<(usize, usize)>> = BinaryHeap::new();
        for c in 0..num_comps {
            if cindeg[c] == 0 {
                ready.push(Reverse((members[c][0], c)));
            }
        }
        let mut plan: Vec<Vec<usize>> = Vec::with_capacity(num_comps);
        while let Some(Reverse((_, c))) = ready.pop() {
            plan.push(std::mem::take(&mut members[c]));
            for &d in &cadj[c] {
                cindeg[d] -= 1;
                if cindeg[d] == 0 {
                    ready.push(Reverse((members[d][0], d)));
                }
            }
        }
        plan
    }

    /// The iterate-to-fixpoint delta-cycle settle (the default scheduler). Uses the
    /// shared [`OSCILLATION_THRESHOLD`] / [`MAX_DELTA_CYCLES`] bounds.
    fn poll_tasks_fixpoint(&mut self) {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        for delta in 0..=MAX_DELTA_CYCLES {
            assert!(
                delta < MAX_DELTA_CYCLES,
                "Delta-cycle limit ({MAX_DELTA_CYCLES}) exceeded — combinational loop not resolved"
            );

            let order = self.visit_order(delta);
            let mut any_dirty = false;
            for &i in &order {
                let task = &mut self.tasks[i];
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
        self.pre_edge_settle::<Domain>();
        self.post_edge_settle::<Domain>(clk);
    }

    /// The **pre-edge settle** half of [`Self::tick_clock`]: set `Domain` to its
    /// pre-edge phase and settle all tasks, without advancing the clock.
    ///
    /// Split out so the differential harness (item 6) can compare settled signal
    /// values *between* phases; ordinary callers use [`Self::tick_clock`].
    pub fn pre_edge_settle<Domain: ClockDomain>(&mut self) {
        crate::set_poll_phase::<Domain>(crate::PollPhase::PreEdge);
        copper_core::types::set_tick_resolving::<Domain>(false);
        self.poll_tasks();
    }

    /// The **clock-edge + post-edge settle** half of [`Self::tick_clock`]: advance
    /// `Domain`'s clock, set its post-edge phase, settle all tasks, and count the
    /// cycle. Must follow [`Self::pre_edge_settle`] for the same domain.
    pub fn post_edge_settle<Domain: ClockDomain>(&mut self, clk: &mut Clock<Domain>) {
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
    use copper_core::{Clock, ClockDomain};

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
        exec.spawn_wired(wired_counter(clk.clone(), out), vec![dirty], vec![]);

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
        exec.spawn_wired(wired_counter(clk.clone(), out), vec![dirty_for_exec], vec![]);

        exec.tick_clock(&mut clk);
        assert!(!dirty_probe.take(), "dirty flag must be clear after settling");
    }

    #[test]
    fn spawn_wired_combinational_passthrough_follows_counter() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let (counter_out, counter_in) = wire::<u8, ()>(0);
        let counter_dirty = counter_out.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), counter_out), vec![counter_dirty], vec![]);

        let (pass_out, pass_in) = wire::<u8, ()>(0);
        let pass_dirty = pass_out.dirty_handle();
        let pass_reads = vec![counter_in.wire_id()];
        exec.spawn_wired(wired_passthrough(counter_in, pass_out), vec![pass_dirty], pass_reads);

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
            vec![],
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
        exec.spawn_wired(wired_counter(clk.clone(), out), vec![dirty], vec![]);

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
        exec.spawn_wired(wired_counter(clk.clone(), out), vec![], vec![]); // no dirty tracking

        exec.tick_clock(&mut clk);
        assert_eq!(in_.read(), 1, "module ran even without dirty tracking");
    }

    #[test]
    fn poll_order_does_not_change_cross_task_settle() {
        // counter → passthrough is the poll-order-sensitive case: under a reversed
        // order the consumer (passthrough) is polled before the producer (counter)
        // within a delta cycle, so it reads a stale value and needs another pass.
        // The settled value after tick_clock must be identical regardless.
        // Pinned to Fixpoint: the [`PollOrder`] knob only affects the fixpoint
        // settle (the levelized scheduler computes its own canonical order and
        // ignores it), so this guards the oracle's poll-order independence.
        use super::PollOrder;
        fn run(order: PollOrder) -> Vec<u8> {
            let mut clk = Clock::<TestClk>::new();
            let mut exec = HardwareExecutor::new().with_scheduler_mode(SchedulerMode::Fixpoint);
            exec.set_poll_order(order);

            let (counter_out, counter_in) = wire::<u8, ()>(0);
            let cd = counter_out.dirty_handle();
            exec.spawn_wired(wired_counter(clk.clone(), counter_out), vec![cd], vec![]);

            let (pass_out, pass_in) = wire::<u8, ()>(0);
            let pd = pass_out.dirty_handle();
            let pass_reads = vec![counter_in.wire_id()];
            exec.spawn_wired(wired_passthrough(counter_in, pass_out), vec![pd], pass_reads);

            (0..5)
                .map(|_| {
                    exec.tick_clock(&mut clk);
                    pass_in.read()
                })
                .collect()
        }

        let baseline = run(PollOrder::Insertion);
        assert_eq!(baseline, vec![1, 2, 3, 4, 5]);
        assert_eq!(run(PollOrder::Reversed), baseline, "reversed poll order diverged");
        for seed in [1u64, 7, 42, 1234] {
            assert_eq!(
                run(PollOrder::Seeded(seed)),
                baseline,
                "seeded poll order {seed} diverged"
            );
        }
    }

    #[test]
    fn spawn_wired_two_independent_counters_run_concurrently() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new();

        let (out_a, in_a) = wire::<u8, ()>(0);
        let dirty_a = out_a.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), out_a), vec![dirty_a], vec![]);

        let (out_b, in_b) = wire::<u8, ()>(0);
        let dirty_b = out_b.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), out_b), vec![dirty_b], vec![]);

        exec.tick_clock(&mut clk);
        assert_eq!(in_a.read(), 1);
        assert_eq!(in_b.read(), 1);

        exec.tick_clock(&mut clk);
        assert_eq!(in_a.read(), 2);
        assert_eq!(in_b.read(), 2);
    }

    // ── levelized scheduler (item 6, phase 2) ─────────────────────────────────

    use super::SchedulerMode;

    /// The counter→passthrough chain — the poll-order-sensitive case — must settle
    /// to identical values under the levelized scheduler as under fixpoint, in a
    /// single topo-ordered pass (producer polled before consumer).
    #[test]
    fn levelized_settles_comb_chain_like_fixpoint() {
        fn run(mode: SchedulerMode) -> Vec<u8> {
            let mut clk = Clock::<TestClk>::new();
            let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);

            let (counter_out, counter_in) = wire::<u8, ()>(0);
            let cd = counter_out.dirty_handle();
            exec.spawn_wired(wired_counter(clk.clone(), counter_out), vec![cd], vec![]);

            let (pass_out, pass_in) = wire::<u8, ()>(0);
            let pd = pass_out.dirty_handle();
            let reads = vec![counter_in.wire_id()];
            exec.spawn_wired(wired_passthrough(counter_in, pass_out), vec![pd], reads);

            (0..5)
                .map(|_| {
                    exec.tick_clock(&mut clk);
                    pass_in.read()
                })
                .collect()
        }

        assert_eq!(run(SchedulerMode::Fixpoint), vec![1, 2, 3, 4, 5]);
        assert_eq!(
            run(SchedulerMode::Levelized),
            run(SchedulerMode::Fixpoint),
            "levelized diverged from fixpoint on the comb chain"
        );
    }

    /// A reversed spawn order (consumer spawned before producer) is the case that
    /// forces the fixpoint loop to take an extra pass; the levelized topo sort must
    /// reorder them and still settle in one pass to the same values.
    #[test]
    fn levelized_reorders_consumer_before_producer() {
        fn run(mode: SchedulerMode) -> Vec<u8> {
            let mut clk = Clock::<TestClk>::new();
            let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);

            // Spawn the consumer FIRST (spawn index 0), producer second.
            let (counter_out, counter_in) = wire::<u8, ()>(0);
            let (pass_out, pass_in) = wire::<u8, ()>(0);
            let pd = pass_out.dirty_handle();
            let reads = vec![counter_in.wire_id()];
            exec.spawn_wired(wired_passthrough(counter_in, pass_out), vec![pd], reads);

            let cd = counter_out.dirty_handle();
            exec.spawn_wired(wired_counter(clk.clone(), counter_out), vec![cd], vec![]);

            (0..4)
                .map(|_| {
                    exec.tick_clock(&mut clk);
                    pass_in.read()
                })
                .collect()
        }

        assert_eq!(
            run(SchedulerMode::Levelized),
            run(SchedulerMode::Fixpoint),
            "levelized diverged when the consumer was spawned before the producer"
        );
    }

    /// The graph is rebuilt after a spawn: a levelized executor that has already
    /// ticked (building the order) must incorporate a task spawned afterward.
    #[test]
    fn levelized_rebuilds_graph_after_late_spawn() {
        let mut clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(SchedulerMode::Levelized);

        let (counter_out, counter_in) = wire::<u8, ()>(0);
        let cd = counter_out.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), counter_out), vec![cd], vec![]);

        // Build the order and advance once with only the counter present.
        exec.tick_clock(&mut clk);

        // Now add the passthrough consumer; the cached order must be invalidated.
        let (pass_out, pass_in) = wire::<u8, ()>(0);
        let pd = pass_out.dirty_handle();
        let reads = vec![counter_in.wire_id()];
        exec.spawn_wired(wired_passthrough(counter_in, pass_out), vec![pd], reads);

        exec.tick_clock(&mut clk); // counter=2 this cycle
        assert_eq!(pass_in.read(), 2, "late-spawned consumer must track the producer");
    }

    // A combinational feedback pair `a = b + 1`, `b = a` — a genuine (non-convergent)
    // combinational loop: it forms one multi-task SCC that never reaches a fixed
    // point, so the per-SCC iteration must trip the same oscillation guard the
    // fixpoint settle does, rather than spin forever.
    fn inc_u8(input: In<u8>, out: Out<u8>) -> HardwareModule<impl std::future::Future<Output = ()>> {
        HardwareModule::__new(async move {
            loop {
                out.write(input.read().wrapping_add(1));
                crate::delta_yield().await;
            }
        })
    }
    fn buf_u8(input: In<u8>, out: Out<u8>) -> HardwareModule<impl std::future::Future<Output = ()>> {
        HardwareModule::__new(async move {
            loop {
                out.write(input.read());
                crate::delta_yield().await;
            }
        })
    }
    // `out = ext | fb` — with `fb` fed back from `out` this is a set-dominant latch:
    // a *convergent* combinational loop (monotone), legal and iterated, never rejected.
    fn or_u8(ext: In<u8>, fb: In<u8>, out: Out<u8>) -> HardwareModule<impl std::future::Future<Output = ()>> {
        HardwareModule::__new(async move {
            loop {
                out.write(ext.read() | fb.read());
                crate::delta_yield().await;
            }
        })
    }

    #[test]
    #[should_panic(expected = "form a combinational cycle")]
    fn levelized_oscillating_scc_trips_threshold() {
        let mut exec = HardwareExecutor::new().with_scheduler_mode(SchedulerMode::Levelized);

        let (a_out, a_in) = wire::<u8, ()>(0);
        let (b_out, b_in) = wire::<u8, ()>(0);

        let ad = a_out.dirty_handle();
        let a_reads = vec![b_in.wire_id()];
        exec.spawn_wired(inc_u8(b_in, a_out), vec![ad], a_reads);

        let bd = b_out.dirty_handle();
        let b_reads = vec![a_in.wire_id()];
        exec.spawn_wired(buf_u8(a_in, b_out), vec![bd], b_reads);

        // levelized: SCC {inc, buf} iterated → non-convergent → structural panic
        // naming the whole cycle (item-6 phase 5).
        exec.poll_tasks();
    }

    /// A 3-module oscillating cycle (`a = c + 1`, `b = a`, `c = b`) — the structural
    /// error must name the *whole* SCC, not just one task (item-6 phase 5).
    #[test]
    #[should_panic(expected = "form a combinational cycle")]
    fn levelized_three_module_cycle_reports_whole_scc() {
        let mut exec = HardwareExecutor::new().with_scheduler_mode(SchedulerMode::Levelized);

        let (a_out, a_in) = wire::<u8, ()>(0);
        let (b_out, b_in) = wire::<u8, ()>(0);
        let (c_out, c_in) = wire::<u8, ()>(0);

        let ad = a_out.dirty_handle();
        let a_reads = vec![c_in.wire_id()];
        exec.spawn_wired(inc_u8(c_in, a_out), vec![ad], a_reads); // a = c + 1

        let bd = b_out.dirty_handle();
        let b_reads = vec![a_in.wire_id()];
        exec.spawn_wired(buf_u8(a_in, b_out), vec![bd], b_reads); // b = a

        let cd = c_out.dirty_handle();
        let c_reads = vec![b_in.wire_id()];
        exec.spawn_wired(buf_u8(b_in, c_out), vec![cd], c_reads); // c = b  → closes the cycle

        exec.poll_tasks();
    }

    /// Static detection: a convergent combinational loop (an OR set-latch) is
    /// *reported* by `comb_cycles` yet remains **legal** — it simulates without a
    /// panic (detection is not rejection; item-6 phase 5).
    #[test]
    fn comb_cycles_reports_convergent_latch_without_rejecting_it() {
        let mut exec = HardwareExecutor::new().with_scheduler_mode(SchedulerMode::Levelized);

        let (ext_drv, ext_in) = wire::<u8, ()>(0);
        let (a_out, a_in) = wire::<u8, ()>(0);
        let (b_out, b_in) = wire::<u8, ()>(0);

        let ad = a_out.dirty_handle();
        let or_reads = vec![ext_in.wire_id(), b_in.wire_id()];
        let b_feedback = b_in.clone();
        exec.spawn_wired(or_u8(ext_in, b_feedback, a_out), vec![ad], or_reads);

        let bd = b_out.dirty_handle();
        let buf_reads = vec![a_in.wire_id()];
        let a_probe = a_in.clone();
        exec.spawn_wired(buf_u8(a_in, b_out), vec![bd], buf_reads);

        // Detected as exactly one combinational cycle of two modules.
        let cycles = exec.comb_cycles();
        assert_eq!(cycles.len(), 1, "the latch is one combinational cycle");
        assert_eq!(cycles[0].len(), 2, "two modules form the cycle");

        // …yet it is legal: the settle converges (monotone OR), no panic.
        ext_drv.write(1);
        exec.poll_tasks();
        assert_eq!(a_probe.read(), 1, "set-latch latches the input high");
        // Latches: even after ext clears, the bit is held via the feedback.
        ext_drv.write(0);
        exec.poll_tasks();
        assert_eq!(a_probe.read(), 1, "latch holds after the set pulse clears");
    }

    /// An acyclic design (counter → passthrough) has no combinational cycles.
    #[test]
    fn comb_cycles_empty_for_acyclic_design() {
        let clk = Clock::<TestClk>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(SchedulerMode::Levelized);

        let (c_out, c_in) = wire::<u8, ()>(0);
        let cd = c_out.dirty_handle();
        exec.spawn_wired(wired_counter(clk.clone(), c_out), vec![cd], vec![]);

        let (p_out, _p_in) = wire::<u8, ()>(0);
        let pd = p_out.dirty_handle();
        let reads = vec![c_in.wire_id()];
        exec.spawn_wired(wired_passthrough(c_in, p_out), vec![pd], reads);

        assert!(exec.comb_cycles().is_empty(), "acyclic design has no comb cycles");
    }

    #[test]
    #[should_panic(expected = "Combinational loop detected")]
    fn fixpoint_oscillating_loop_trips_threshold() {
        // The same oscillator must be rejected identically under the fixpoint
        // oracle — the levelized SCC guard reproduces the existing behavior.
        // Pinned to Fixpoint (not the env-sensitive default) so it tests the
        // oracle regardless of `COPPER_SCHEDULER`.
        let mut exec = HardwareExecutor::new().with_scheduler_mode(SchedulerMode::Fixpoint);

        let (a_out, a_in) = wire::<u8, ()>(0);
        let (b_out, b_in) = wire::<u8, ()>(0);

        let ad = a_out.dirty_handle();
        exec.spawn_wired(inc_u8(b_in, a_out), vec![ad], vec![]);
        let bd = b_out.dirty_handle();
        exec.spawn_wired(buf_u8(a_in, b_out), vec![bd], vec![]);

        exec.poll_tasks();
    }
}
