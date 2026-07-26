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

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use copper_core::port::In;

thread_local! {
    static CALL_ID: Cell<u64> = Cell::new(0);
}

/// Called once per `tick_clock()` invocation. Internal to this crate — nothing
/// outside `HardwareExecutor` needs to bump this.
pub(crate) fn bump_call_id() {
    CALL_ID.with(|c| c.set(c.get() + 1));
}

fn current_call_id() -> u64 {
    CALL_ID.with(|c| c.get())
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

    pub struct SyncedRead<'a, T, D> {
        port: &'a In<T, D>,
        tracker: &'a ReadTracker,
        current_wrap: u64,
    }

    impl<'a, T: Clone, D> Future for SyncedRead<'a, T, D> {
        type Output = T;
        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<T> {
            let pre_edge = crate::is_pre_edge();
            let wrapped_since = self.current_wrap > self.tracker.wrap_at_last_success.get();
            let same_call = current_call_id() == self.tracker.last_success_call_id.get();

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
            // and must NOT be deferred. `last_success_pre_edge` records which kind
            // this port's reader is, so only pre-edge readers are held back.
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
                && (same_call || (!pre_edge && self.tracker.last_success_pre_edge.get()));
            if block {
                Poll::Pending
            } else {
                self.tracker.last_success_call_id.set(current_call_id());
                self.tracker.wrap_at_last_success.set(self.current_wrap);
                self.tracker.last_success_pre_edge.set(pre_edge);
                Poll::Ready(self.port.read())
            }
        }
    }

    /// `wrap` is the enclosing tick-bearing loop's wrap counter (macro-injected,
    /// incremented once at the top of the loop, unconditionally).
    pub fn synced_read<'a, T: Clone, D>(
        port: &'a In<T, D>,
        tracker: &'a ReadTracker,
        wrap: u64,
    ) -> SyncedRead<'a, T, D> {
        SyncedRead { port, tracker, current_wrap: wrap }
    }
}
