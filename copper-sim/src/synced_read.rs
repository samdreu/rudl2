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
//! The rule: a read blocks only if BOTH (a) the caller's loop has wrapped since
//! this exact port's last successful read, AND (b) no real `tick_clock()` call
//! has happened since that last success. Reading the same port multiple times
//! within one iteration never blocks (the wrap count hasn't moved), and reading
//! it again after a genuine tick never blocks either (the call id has moved) —
//! it only fires for the "two logical iterations compressed into one
//! `tick_clock()` call" case, which is the actual bug (see `dual_port_ram`).

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
    }

    impl ReadTracker {
        pub fn new() -> Self {
            Self {
                last_success_call_id: Cell::new(0),
                wrap_at_last_success: Cell::new(0),
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
            let call_id = current_call_id();
            let wrapped_since = self.current_wrap > self.tracker.wrap_at_last_success.get();
            let same_call_as_last_success = call_id == self.tracker.last_success_call_id.get();

            if wrapped_since && same_call_as_last_success {
                // The loop wrapped since we last read here, but no new
                // tick_clock() call has happened since — this would be a
                // premature re-read.
                Poll::Pending
            } else {
                self.tracker.last_success_call_id.set(call_id);
                self.tracker.wrap_at_last_success.set(self.current_wrap);
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
