//! Internal machinery for `#[hardware(sequential)]`'s per-read freshness check —
//! the replacement for the old `pre_edge_barrier` blanket-loop barrier.
//!
//! Everything in this module is `#[doc(hidden)]` and reached only via
//! fully-qualified paths from macro-generated code (`copper-macros`'s
//! `SyncedReadInjector`). It is not meant to be used by hand — there is no
//! ergonomic, safe way to drive the wrap counter correctly outside the macro's
//! own AST rewrite, since it must be incremented exactly once per loop
//! iteration for every tick-bearing loop in the function, and used consistently
//! across every read in scope.
//!
//! The rule: a loop-top read (one positioned before any tick in its iteration,
//! so it last settled at a pre-edge) is held back on loop re-entry until the next
//! pre-edge settle — the registering clock edge, matching the transpiled FSM's
//! input register. A read positioned after a tick settles at a post-edge and is
//! never deferred (it consumes the value that edge produced). Reading the same
//! port multiple times within one iteration never blocks (the wrap count hasn't
//! moved). This makes multi-tick loops sample inputs on the same schedule as real
//! synchronous hardware (see EXECUTION_MODEL_RECONCILIATION.md).
//!
//! A second term (`same_call`) is retained from the original guard: a wrapped
//! re-read in the *same* `tick_clock` call is always held back. This both handles
//! the same-cycle double-read (`dual_port_ram`) and — critically — prevents an
//! infinite loop when an iteration executes no tick at all (all branches skipped):
//! without it the body would re-run forever inside a single `poll()`. Spin
//! iterations share a `call_id`, so the re-read suspends the task until the next
//! `tick_clock`.

use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use copper_core::port::In;
use copper_core::ClockDomain;

thread_local! {
    /// Keyed per clock domain (by `TypeId`) so bumping one domain's call id on its
    /// `tick_clock` cannot perturb another domain's read-freshness tracking.
    static CALL_ID: RefCell<HashMap<TypeId, u64>> = RefCell::new(HashMap::new());
}

/// Called once per `tick_clock()` invocation, for the domain being ticked.
/// Internal to this crate — nothing outside `HardwareExecutor` needs to bump this.
pub(crate) fn bump_call_id<Domain: ClockDomain>() {
    CALL_ID.with(|c| {
        let mut map = c.borrow_mut();
        let entry = map.entry(TypeId::of::<Domain>()).or_insert(0);
        *entry += 1;
    });
}

fn current_call_id<Domain: ClockDomain>() -> u64 {
    CALL_ID.with(|c| c.borrow().get(&TypeId::of::<Domain>()).copied().unwrap_or(0))
}

#[doc(hidden)]
pub mod __private {
    use super::*;

    /// One of these per `In<T, D>` parameter, injected by the macro at the top
    /// of the function body. Shared across every `.read()` call site for that
    /// parameter (confirmed sufficient — see the reasoning above and
    /// `tests/prototype_synced_read.rs`'s `max_select` case in the commit that
    /// introduced this: the wrap counter's strict inequality already makes
    /// same-iteration re-reads a no-op regardless of whether trackers are
    /// shared per port or split per call site).
    pub struct ReadTracker {
        last_success_call_id: Cell<u64>,
        wrap_at_last_success: Cell<u64>,
        /// Whether this port's last successful read happened during a pre-edge
        /// settle. A read positioned *before* any tick in its loop iteration
        /// settles at pre-edge (it samples the registering edge); a read *after*
        /// a tick settles at post-edge (it consumes the value the edge just
        /// produced). This flag is what tells the two apart on re-entry — only
        /// the former should be held back to the next pre-edge.
        last_success_pre_edge: Cell<bool>,
    }

    impl ReadTracker {
        pub fn new() -> Self {
            Self {
                last_success_call_id: Cell::new(0),
                wrap_at_last_success: Cell::new(0),
                last_success_pre_edge: Cell::new(false),
            }
        }
    }

    /// `D` is the port's own declared clock domain (whatever `In<T, D>` says);
    /// `TickDomain` is the clock whose phase/call-id actually gates this read.
    /// They coincide for an ordinary port on a single- or multi-clock module (the
    /// port's domain is ticked by one of this function's own `Clock<D>`
    /// parameters), but diverge for a `#[hardware(synchronizer)]` module's
    /// sanctioned foreign-domain input: nothing in this function ever ticks that
    /// port's nominal domain (it belongs to some other module entirely), so the
    /// macro passes the synchronizer's own native clock as `TickDomain` instead —
    /// the read is gated by the reactor that is actually executing it, not by the
    /// port's CDC label. See `copper-macros::inject_synced_reads` for how the two
    /// are chosen per port.
    pub struct SyncedRead<'a, T, D, TickDomain> {
        port: &'a In<T, D>,
        tracker: &'a ReadTracker,
        current_wrap: u64,
        /// The `call_id` of the most recent PRE-EDGE read success *this wrap*
        /// (shared across every `In` parameter's reads in the function — see
        /// `synced_read` and `copper-macros::inject_synced_reads`), or `None` if
        /// no read has succeeded at pre-edge yet this wrap. Scoped to a specific
        /// call_id (not just "anytime this wrap") deliberately: a read whose own
        /// history is post-edge-type (like `bsg_dff_en`'s `data.read()` right
        /// after its only tick) still needs deferring if some EARLIER read in
        /// THIS SAME `tick_clock` call already consumed this edge's data at
        /// pre-edge — the "leading phase" case (`sipo_block`'s `w1`/`w2`/`w3`,
        /// each following a loop-top-or-earlier read separated by its own tick).
        /// But a read reached without crossing any NEW tick since that earlier
        /// success — e.g. `det_010_awaits`'s `if in_i.read() == One` immediately
        /// after a `while in_i.read() == Zero` loop exits, no tick in between —
        /// must NOT defer just because some read succeeded at pre-edge many
        /// calls ago; comparing call_ids is what tells "still the same edge, a
        /// different call site" apart from "a genuinely later edge."
        leading_pre_edge_call_id: &'a Cell<Option<u64>>,
        _tick_domain: PhantomData<TickDomain>,
    }

    impl<'a, T: Clone, D, TickDomain: ClockDomain> Future for SyncedRead<'a, T, D, TickDomain> {
        type Output = T;
        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<T> {
            let pre_edge = crate::is_pre_edge::<TickDomain>();
            let call_id = current_call_id::<TickDomain>();
            let wrapped_since = self.current_wrap > self.tracker.wrap_at_last_success.get();
            let same_call = call_id == self.tracker.last_success_call_id.get();
            let leading_this_call = self.leading_pre_edge_call_id.get() == Some(call_id);

            // A read positioned *before* any tick in its loop iteration samples
            // its input at the registering clock edge — the pre-edge settle of the
            // next tick_clock, exactly where the transpiled FSM's input register
            // captures. Such a read settles at pre-edge; on loop re-entry it is
            // first re-polled during a post-edge settle (a tick always resolves
            // post-edge), and must be held back to the next pre-edge so it samples
            // one edge later. Firing at that post-edge reads one cycle early — the
            // multi-tick discrepancy in EXECUTION_MODEL_RECONCILIATION.md, shown
            // (probe/mac vs independent hand-written Verilog) to be a simulator
            // bug for 2+-tick loops (1-tick loops already resume a pre-edge later).
            //
            // A read *after* a tick (e.g. `count = count + step.read()`) instead
            // consumes the value the edge just produced; it settles at post-edge
            // and must NOT be deferred, UNLESS some earlier read in THIS SAME
            // `tick_clock` call already succeeded at pre-edge (`leading_this_call`)
            // — that earlier read already consumed this cycle's data, so a
            // *later* post-tick read that crossed its own tick to get here (e.g.
            // `sipo_block`'s `w1` following `w0`) needs a cycle of its own too,
            // exactly like a loop-top read does on re-entry. `bsg_dff_en`'s
            // `data.read()` — the first and only read in an iteration whose tick
            // is the loop's first statement — never sees this, so it fires
            // un-deferred as before. `last_success_pre_edge` records which kind
            // THIS port's own reader is; `leading_this_call` additionally
            // captures whether some OTHER read already claimed this same edge —
            // scoped to the current call_id specifically, so a read reached
            // without crossing a new tick since that earlier success (e.g. a
            // check immediately following a `while` loop's exit, no tick in
            // between) is correctly treated as the same instant, not deferred.
            // Same-iteration re-reads leave the wrap counter unmoved
            // (wrapped_since == false) and never block.
            //
            // The `same_call` term is the original guard, kept because it also
            // prevents an infinite loop: if a loop iteration executes *no* tick
            // (e.g. all branches skipped), the body would otherwise re-run inside a
            // single `poll()` forever. Spin iterations share a `call_id`, so a
            // wrapped re-read in the same call is held back regardless of phase,
            // suspending the task until the next `tick_clock`.
            let block = wrapped_since
                && (same_call
                    || (!pre_edge
                        && (self.tracker.last_success_pre_edge.get() || leading_this_call)));
            if block {
                Poll::Pending
            } else {
                self.tracker.last_success_call_id.set(call_id);
                self.tracker.wrap_at_last_success.set(self.current_wrap);
                self.tracker.last_success_pre_edge.set(pre_edge);
                if pre_edge {
                    self.leading_pre_edge_call_id.set(Some(call_id));
                }
                Poll::Ready(self.port.read())
            }
        }
    }

    /// `wrap` is the enclosing tick-bearing loop's wrap counter (macro-injected,
    /// incremented once at the top of the loop, unconditionally). `leading` is a
    /// per-function flag reset to `None` alongside `wrap` (macro-injected) and
    /// shared by every `In` parameter's reads — see `SyncedRead`. `TickDomain` is
    /// always turbofished explicitly by the macro (see `SyncedRead` above) —
    /// nothing here ties it to `D`, so it cannot be inferred from the arguments.
    pub fn synced_read<'a, T: Clone, D, TickDomain: ClockDomain>(
        port: &'a In<T, D>,
        tracker: &'a ReadTracker,
        wrap: u64,
        leading: &'a Cell<Option<u64>>,
    ) -> SyncedRead<'a, T, D, TickDomain> {
        SyncedRead {
            port,
            tracker,
            current_wrap: wrap,
            leading_pre_edge_call_id: leading,
            _tick_domain: PhantomData,
        }
    }
}
