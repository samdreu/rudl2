//! Poll-order fuzzer (gate G3, `design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`).
//!
//! CLAUDE.md / `SYNCHRONOUS_SEMANTICS.md` invariant: **a well-formed design must
//! simulate identically under any Rust async poll order.** The executor's
//! `poll_tasks` visits tasks in spawn order by default; this fuzzer re-runs
//! representative *multi-task* designs under reversed and randomized visit orders
//! (`PollOrder`) and asserts the observable traces are byte-identical to the
//! insertion-order baseline.
//!
//! This is a regression guard *before* the item-3 refactor. Item 6 (levelized
//! scheduling) later makes the order canonical, at which point this guard becomes
//! moot — until then, any change that makes a settled value depend on poll order
//! is a real bug and must fail here.
//!
//! Futures are single-use, so each poll order rebuilds the design from scratch;
//! the closures below are the "design + stimulus", parameterized by `PollOrder`.

use copper_core::port::{wire, In, Out};
use copper_core::types::{Bits, Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, PollOrder};

struct ClkA;
impl ClockDomain for ClkA {}
struct ClkB;
impl ClockDomain for ClkB {}

/// The adversarial orders every scenario is checked under.
fn orders() -> Vec<PollOrder> {
    let mut v = vec![PollOrder::Insertion, PollOrder::Reversed];
    for seed in [1u64, 2, 7, 42, 1234, 0xDEAD_BEEF] {
        v.push(PollOrder::Seeded(seed));
    }
    v
}

/// Assert every order produces the same trace as the insertion-order baseline.
fn assert_order_independent(name: &str, run: impl Fn(PollOrder) -> Vec<u128>) {
    let baseline = run(PollOrder::Insertion);
    for order in orders() {
        let got = run(order);
        assert_eq!(
            got, baseline,
            "{name}: poll order {order:?} produced a different trace than insertion order\n  \
             baseline = {baseline:?}\n  got      = {got:?}"
        );
    }
}

// ── modules (real #[hardware] macro path) ─────────────────────────────────────

#[hardware(sequential)]
async fn counter_a(clk: Clock<ClkA>, out: Out<Bits<8>, ClkA>) {
    let mut v = Bits::<8>::from_lit::<0>();
    loop {
        out.write(v.clone());
        clk.tick().await;
        v = v + Bits::<8>::from_u8(1);
    }
}

#[hardware(sequential)]
async fn counter_b(clk: Clock<ClkB>, out: Out<Bits<8>, ClkB>) {
    let mut v = Bits::<8>::from_lit::<0>();
    loop {
        out.write(v.clone());
        clk.tick().await;
        v = v + Bits::<8>::from_u8(1);
    }
}

/// Combinational consumer: `out = in + 1`. Wired downstream of a register, this is
/// the poll-order-sensitive case — under a reversed order the consumer is polled
/// before its producer within a delta cycle and must re-settle.
#[hardware(combinational)]
fn add_one(in_i: In<Bits<8>, ClkA>, out: Out<Bits<8>, ClkA>) {
    out.write(in_i.read() + Bits::<8>::from_u8(1));
}

// ── scenarios ─────────────────────────────────────────────────────────────────

/// A 2-level combinational chain fed by a register: counter → add_one → add_one.
/// Spawned deliberately in *dependency* order so that `Reversed` polls the deepest
/// consumer first — the maximal settle-order stress.
#[test]
fn combinational_chain_is_poll_order_independent() {
    fn run(order: PollOrder) -> Vec<u128> {
        let mut clk = Clock::<ClkA>::new();
        let mut exec = HardwareExecutor::new();
        exec.set_poll_order(order);

        let (c_out, c_in) = wire::<Bits<8>, ClkA>(Bits::from_lit::<0>());
        let (a_out, a_in) = wire::<Bits<8>, ClkA>(Bits::from_lit::<0>());
        let (b_out, b_in) = wire::<Bits<8>, ClkA>(Bits::from_lit::<0>());

        let cd = c_out.dirty_handle();
        let ad = a_out.dirty_handle();
        let bd = b_out.dirty_handle();

        exec.spawn_wired(counter_a(clk.clone(), c_out), vec![cd]);
        exec.spawn_wired(add_one(c_in, a_out), vec![ad]);
        exec.spawn_wired(add_one(a_in, b_out), vec![bd]);

        (0..6)
            .map(|_| {
                exec.tick_clock(&mut clk);
                // record the deepest node — it only settles if the whole chain did
                b_in.read().as_u128()
            })
            .collect()
    }

    // counter reads 1,2,3… (post-edge convention); +2 through the chain.
    assert_eq!(run(PollOrder::Insertion), vec![3, 4, 5, 6, 7, 8]);
    assert_order_independent("combinational_chain", run);
}

/// Two independent clock domains ticked in an uneven interleaving, each driving
/// its own counter. Exercises poll-order independence across a multi-domain
/// executor (the direction item 4 generalizes to interleave-independence).
#[test]
fn multi_domain_counters_are_poll_order_independent() {
    // 0 = tick A, 1 = tick B.
    let schedule = [0u8, 0, 1, 0, 1, 1, 0, 1, 0, 0];

    fn run(order: PollOrder, schedule: &[u8]) -> Vec<u128> {
        let mut clk_a = Clock::<ClkA>::new();
        let mut clk_b = Clock::<ClkB>::new();
        let mut exec = HardwareExecutor::new();
        exec.set_poll_order(order);

        let (a_out, a_in) = wire::<Bits<8>, ClkA>(Bits::from_lit::<0>());
        let (b_out, b_in) = wire::<Bits<8>, ClkB>(Bits::from_lit::<0>());
        let ad = a_out.dirty_handle();
        let bd = b_out.dirty_handle();
        exec.spawn_wired(counter_a(clk_a.clone(), a_out), vec![ad]);
        exec.spawn_wired(counter_b(clk_b.clone(), b_out), vec![bd]);

        let mut trace = Vec::new();
        for &which in schedule {
            if which == 0 {
                exec.tick_clock(&mut clk_a);
            } else {
                exec.tick_clock(&mut clk_b);
            }
            // record both domains' observable state after each tick
            trace.push(a_in.read().as_u128());
            trace.push(b_in.read().as_u128());
        }
        trace
    }

    assert_order_independent("multi_domain", |order| run(order, &schedule));
}
