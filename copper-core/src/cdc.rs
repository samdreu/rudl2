//! # Clock-domain-crossing (CDC) safety — executable specification & audit
//!
//! This module has no code. Its doctests **are** the specification: the
//! `compile_fail` blocks are compiled by `cargo test` and the suite fails if any
//! of them ever *starts* compiling, so the guarantees below cannot silently
//! regress.
//!
//! ## Mechanism (audited 2026-07-14)
//!
//! Signals and clocks carry a **phantom domain type parameter** —
//! [`Clock<D>`](crate::Clock), [`In<T, D>`](crate::port::In),
//! [`Out<T, D>`](crate::port::Out) — where `D: ClockDomain`. Connecting a signal
//! of one domain to a port of another is a *nominal type mismatch* (`E0308`),
//! rejected at `cargo build`.
//!
//! Note this is **phantom-type-based domain tagging**, checked by nominal typing.
//! The separate "one writer per wire" property (`Out` is not `Clone`) is what the
//! ownership model buys; it prevents multiple drivers, not domain crossings.
//! Framing the *domain* guarantee as "ownership-based" (as the README does) is
//! imprecise — see the audit note in `TRANSPILATION_TODO.md`.
//!
//! ## What IS guaranteed
//!
//! **Every clock-domain crossing is a visible, typed module boundary.** You
//! cannot wire two domains together implicitly; a crossing can only occur inside
//! a module whose *signature* declares ports of differing domains (a
//! synchronizer). The crossing point can never be hidden.
//!
//! ## What is NOT guaranteed by the *types alone*
//!
//! 1. **Synchronizer correctness — Gap 1, now closed at the macro layer.** The
//!    types alone don't stop a mixed-domain function from forwarding a foreign
//!    value to a native output with no synchronizer (the plain-`fn` example under
//!    "GAP 1" below still compiles). This is closed by the CDC check in
//!    `#[hardware(sequential)]`: a *regular* hardware module may not even declare
//!    a foreign-domain port. The only thing that may is a
//!    `#[hardware(synchronizer)]` module (e.g. `copper::sync_2ff`), which is a
//!    real, provided two-flip-flop synchronizer. So there is no user-written
//!    crossing logic left to get wrong — crossings go through a correct-by-
//!    construction library primitive. (The plain-`fn` doctests below are *not*
//!    `#[hardware]`, so they show the type-only behavior, not the enforced one.)
//! 2. **Clock construction — Gap 2, now closed at the macro layer.** `Clock` is
//!    an instance capability (`Arc<ClockState>`): a fabricated clock is a distinct
//!    instance the testbench never advances, so a module ticking it would *hang*
//!    (not silently mix domains — data still flows only through domain-typed
//!    ports). The types alone don't stop a module calling `Clock::<D>::new()`, but
//!    the `#[hardware]` macro rejects any clock construction in a module body: a
//!    module receives its clocks as parameters and may only `.clone()` them (a
//!    clone shares the same real clock). Testbenches, which are not `#[hardware]`,
//!    create clocks normally.
//!
//! ---
//!
//! ## GUARANTEED — rejected at compile time
//!
//! Each case below is asserted in `copper-core/tests/ui/fail/`, one file per rule,
//! with the compiler's message pinned in the adjacent `.stderr`. They were
//! `compile_fail` doctests until 2026-08-26; that form asserts only that the snippet
//! failed to compile, so a typo or a renamed API satisfied it exactly as well as the
//! guarantee, and the error CODE — `E0308` for a domain crossing, `E0382` for the
//! single-driver move, `E0599` for `RegOut` not being `Clone` — was never checked at
//! all. Run them with `cargo test -p copper-core --test cdc_ui`.
//!
//! Connecting a `Fast`-domain input wire to a `Slow`-domain input port:
//!
//! Asserted, with its compiler message pinned, in
//! `copper-core/tests/ui/fail/in_port_foreign_domain.rs`. Shown here rather than run:
//! `compile_fail` would pass whatever the reason for failing.
//!
//! ```ignore
//! use copper_core::port::{wire, In, Out};
//! use copper_core::{Clock, ClockDomain, Logic};
//! struct Fast; impl ClockDomain for Fast {}
//! struct Slow; impl ClockDomain for Slow {}
//! fn slow_module(_c: Clock<Slow>, _d: In<Logic, Slow>, _q: Out<Logic, Slow>) {}
//!
//! let (_o, fast_in) = wire::<Logic, Fast>(Logic::Zero);
//! let (slow_q, _obs) = wire::<Logic, Slow>(Logic::Zero);
//! // E0308: expected `In<Logic, Slow>`, found `In<Logic, Fast>`
//! slow_module(Clock::<Slow>::new(), fast_in, slow_q);
//! ```
//!
//! Connecting a `Fast`-domain output wire to a `Slow`-domain output port:
//!
//! Asserted, with its compiler message pinned, in
//! `copper-core/tests/ui/fail/out_port_foreign_domain.rs`. Shown here rather than run:
//! `compile_fail` would pass whatever the reason for failing.
//!
//! ```ignore
//! use copper_core::port::{wire, Out};
//! use copper_core::{Clock, ClockDomain, Logic};
//! struct Fast; impl ClockDomain for Fast {}
//! struct Slow; impl ClockDomain for Slow {}
//! fn slow_module(_c: Clock<Slow>, _q: Out<Logic, Slow>) {}
//!
//! let (fast_out, _fi) = wire::<Logic, Fast>(Logic::Zero);
//! // E0308: expected `Out<Logic, Slow>`, found `Out<Logic, Fast>`
//! slow_module(Clock::<Slow>::new(), fast_out);
//! ```
//!
//! **Multi-driver across domains — unrepresentable, on two independent counts.**
//!
//! There is no behaviour to define here, because the program cannot be written. A
//! wire has exactly one write handle (`Out`/`RegOut` are **not** `Clone`), so a
//! second driver is a move error; and domains are nominal, so a driver from another
//! domain is a type error. Either one alone is fatal.
//!
//! Count 1 — the wire is already moved into its single driver:
//!
//! Asserted, with its compiler message pinned, in
//! `copper-core/tests/ui/fail/out_is_not_clone.rs`. Shown here rather than run:
//! `compile_fail` would pass whatever the reason for failing.
//!
//! ```ignore
//! use copper_core::port::{wire, Out};
//! use copper_core::{Clock, ClockDomain, Logic};
//! struct Fast; impl ClockDomain for Fast {}
//! fn driver_a(_c: Clock<Fast>, _q: Out<Logic, Fast>) {}
//! fn driver_b(_c: Clock<Fast>, _q: Out<Logic, Fast>) {}
//!
//! let (out, _obs) = wire::<Logic, Fast>(Logic::Zero);
//! driver_a(Clock::<Fast>::new(), out);
//! // E0382: use of moved value `out` — a wire has one driver, by move semantics
//! driver_b(Clock::<Fast>::new(), out);
//! ```
//!
//! Count 2 — and even the *attempt* to reach across a domain is a type error, so
//! cloning `Out` (if it were possible) would not help:
//!
//! Asserted, with its compiler message pinned, in
//! `copper-core/tests/ui/fail/second_driver_foreign_domain.rs`. Shown here rather than run:
//! `compile_fail` would pass whatever the reason for failing.
//!
//! ```ignore
//! use copper_core::port::{wire, Out};
//! use copper_core::{Clock, ClockDomain, Logic};
//! struct Fast; impl ClockDomain for Fast {}
//! struct Slow; impl ClockDomain for Slow {}
//! fn slow_driver(_c: Clock<Slow>, _q: Out<Logic, Slow>) {}
//!
//! let (fast_out, _obs) = wire::<Logic, Fast>(Logic::Zero);
//! // E0308: expected `Out<Logic, Slow>`, found `Out<Logic, Fast>`
//! slow_driver(Clock::<Slow>::new(), fast_out);
//! ```
//!
//! `RegOut` carries the same single-driver guarantee as `Out`:
//!
//! Asserted, with its compiler message pinned, in
//! `copper-core/tests/ui/fail/regout_is_not_clone.rs`. Shown here rather than run:
//! `compile_fail` would pass whatever the reason for failing.
//!
//! ```ignore
//! use copper_core::port::registered_wire;
//! use copper_core::{Clock, ClockDomain, Logic};
//! struct Fast; impl ClockDomain for Fast {}
//!
//! let clk = Clock::<Fast>::new();
//! let (regout, _obs) = registered_wire::<Logic, Fast>(&clk, Logic::Zero);
//! let _second = regout.clone(); // error: `RegOut<Logic, Fast>` is not `Clone`
//! ```
//!
//! Passing a `Fast` clock where a `Slow` clock is required:
//!
//! Asserted, with its compiler message pinned, in
//! `copper-core/tests/ui/fail/clock_foreign_domain.rs`. Shown here rather than run:
//! `compile_fail` would pass whatever the reason for failing.
//!
//! ```ignore
//! use copper_core::{Clock, ClockDomain};
//! struct Fast; impl ClockDomain for Fast {}
//! struct Slow; impl ClockDomain for Slow {}
//! fn slow_module(_c: Clock<Slow>) {}
//!
//! slow_module(Clock::<Fast>::new()); // E0308: expected Clock<Slow>, found Clock<Fast>
//! ```
//!
//! ## GUARANTEED — the legal path compiles
//!
//! Same-domain wiring, and a synchronizer generic over its *source* domain that
//! legally accepts a `Fast` input while clocked on `Slow`:
//!
//! ```
//! use copper_core::port::{wire, In, Out};
//! use copper_core::{Clock, ClockDomain, Logic};
//! struct Fast; impl ClockDomain for Fast {}
//! struct Slow; impl ClockDomain for Slow {}
//!
//! // A synchronizer: destination-clocked, source domain is a free parameter.
//! fn sync_2ff<Src: ClockDomain>(_c: Clock<Slow>, _d: In<Logic, Src>, _q: Out<Logic, Slow>) {}
//!
//! let (_fo, fast_in) = wire::<Logic, Fast>(Logic::Zero);   // Fast source
//! let (sync_q, _obs) = wire::<Logic, Slow>(Logic::Zero);   // Slow destination
//! // Legal: the crossing is explicit in the signature (`In<Logic, Fast>` here).
//! sync_2ff(Clock::<Slow>::new(), fast_in, sync_q);
//! ```
//!
//! ## GAP 1 (types only) — closed by the macro check
//!
//! As a plain `fn`, this "synchronizer" does no synchronizing — it forwards a
//! `Fast` value straight to a `Slow` output — yet it compiles, because the types
//! alone only localize the crossing, they don't verify the logic:
//!
//! ```
//! use copper_core::port::{In, Out};
//! use copper_core::{Clock, ClockDomain, Logic};
//! struct Fast; impl ClockDomain for Fast {}
//! struct Slow; impl ClockDomain for Slow {}
//!
//! fn bad_bridge(_c: Clock<Slow>, fast_d: In<Logic, Fast>, slow_q: Out<Logic, Slow>) {
//!     slow_q.write(fast_d.read()); // compiles as a plain fn...
//! }
//! let _ = bad_bridge;
//! ```
//!
//! ...but add `#[hardware(sequential)]` and it is **rejected at compile time** —
//! a regular module may not declare the foreign `In<Logic, Fast>` port. The
//! crossing must instead go through `copper::sync_2ff` (a `#[hardware(synchronizer)]`
//! module), which is the only kind allowed a foreign-domain port.
//!
//! ## GAP 2 — clocks are freely constructible
//!
//! ```
//! use copper_core::{Clock, ClockDomain};
//! struct Fast; impl ClockDomain for Fast {}
//! let _c = Clock::<Fast>::new(); // any code can mint a clock of any domain
//! ```
