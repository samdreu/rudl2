//! Clock-domain-crossing synchronizers — the library primitives.
//!
//! A regular `#[hardware(sequential)]` module is single-domain (enforced by the
//! CDC check in `copper-macros`). To bring a signal from another clock domain
//! into your module's domain, instantiate one of these `#[hardware(synchronizer)]`
//! modules and read its output — never wire a foreign-domain signal into ordinary
//! logic. See the audit and rationale in `copper-core/src/cdc.rs`.
//!
//! Synchronizers are restricted to single-bit `Logic`: a two-flip-flop
//! synchronizer on a multi-bit bus is itself a CDC bug (bits can be sampled
//! mid-transition). Multi-bit crossings need gray coding, a handshake, or an
//! async FIFO — separate constructs, not yet provided.
//!
//! # A regular module may not cross clock domains
//!
//! A foreign-domain port on a `#[hardware(sequential)]` module is a compile error
//! — the crossing must go through a synchronizer instead:
//!
//! ```compile_fail
//! use copper_core::port::{In, Out};
//! use copper_core::{Clock, ClockDomain, Logic};
//! use copper_macros::hardware;
//! struct Fast; impl ClockDomain for Fast {}
//! struct Slow; impl ClockDomain for Slow {}
//!
//! #[hardware(sequential)]
//! async fn bad(clk: Clock<Slow>, fast: In<Logic, Fast>, q: Out<Logic, Slow>) {
//!     loop { q.write(fast.read()); clk.tick().await; }   // rejected: foreign input `fast`
//! }
//! ```
//!
//! # Writing your own synchronizer
//!
//! `#[hardware(synchronizer)]` is the *only* mode allowed a foreign-domain port —
//! an explicit, `unsafe`-like opt-in for when you need something other than the
//! library `sync_2ff` (a deeper pipeline, a handshake, gray coding, …). You take
//! responsibility for the timing correctness inside it.
//!
//! ```
//! use copper_core::port::{In, Out};
//! use copper_core::{Clock, ClockDomain, Logic};
//! use copper_macros::hardware;
//! struct Slow; impl ClockDomain for Slow {}
//!
//! // A three-flip-flop synchronizer for an extra-conservative crossing.
//! #[hardware(synchronizer)]
//! async fn sync_3ff<Src: ClockDomain>(clk: Clock<Slow>, d: In<Logic, Src>, q: Out<Logic, Slow>) {
//!     let (mut a, mut b, mut c) = (Logic::Zero, Logic::Zero, Logic::Zero);
//!     loop {
//!         q.write(c);
//!         clk.tick().await;
//!         c = b; b = a; a = d.read();
//!     }
//! }
//! let _ = sync_3ff::<Slow>;
//! ```

use copper_core::port::{In, Out};
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;

/// Two-flip-flop synchronizer: samples a single-bit signal from source domain
/// `Src` into destination domain `Dst`, adding two `Dst` cycles of latency to let
/// metastability resolve.
///
/// `Src` is a free type parameter, so this accepts a signal tagged with *any*
/// source domain; the output `q` is firmly in the destination domain `Dst`.
#[hardware(synchronizer)]
pub async fn sync_2ff<Src: ClockDomain, Dst: ClockDomain>(
    clk: Clock<Dst>,
    d: In<Logic, Src>,
    q: Out<Logic, Dst>,
) {
    let mut ff1 = Logic::Zero;
    let mut ff2 = Logic::Zero;
    loop {
        q.write(ff2);
        clk.tick().await;
        // ff2 captures the OLD ff1 (ff1 is reassigned after), so the two stages
        // stay distinct — reversing these lines would collapse them into one flop.
        ff2 = ff1;
        ff1 = d.read();
    }
}
