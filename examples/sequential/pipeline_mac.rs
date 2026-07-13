// Demonstrates the structural advantage of multiple clk.tick().await in one loop.
//
// Both modules below compute the same thing: a 3-stage pipeline that produces
// (a * b) + c with a 3-cycle latency. The FSM version requires explicit state
// tracking and inter-stage register declarations. The pipeline version writes
// straight-line code and lets the compiler infer all pipeline registers.
//
// The test harness runs both with identical inputs and asserts identical outputs.

use copper_core::types::{Bits, Clock, ClockDomain};
use copper_core::port::{In, Out, wire};
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

// ─────────────────────────────────────────────────────────────────────────────
// Version A: single clk.tick().await with explicit state machine
//
// To build a 3-stage pipeline this way you need:
//   - an explicit State enum with one variant per stage
//   - register variables for all inter-stage data, declared at the top level
//     (the compiler cannot infer them because they do not live across .await)
//   - a match dispatch on every clock edge
//   - explicit state transitions in every arm
// ─────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
enum Stage { Load, Mul, Out }

async fn mac_fsm(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    b: In<Bits<8>, MainClk>,
    c: In<Bits<8>, MainClk>,
    out: Out<Bits<8>, MainClk>,
) {
    let mut stage = Stage::Load;
    // Inter-stage registers: named explicitly, initialised, managed by hand.
    let mut product: Bits<8> = Bits::from_lit::<0>();
    let mut c_latch: Bits<8> = Bits::from_lit::<0>(); // pass-through for c
    let mut result:  Bits<8> = Bits::from_lit::<0>();

    loop {
        match stage {
            Stage::Load => {
                product = a.read() * b.read();
                c_latch = c.read();      // must be explicitly latched alongside product
                stage   = Stage::Mul;
            }
            Stage::Mul => {
                result = product.clone() + c_latch.clone();
                stage  = Stage::Out;
            }
            Stage::Out => {
                out.write(result.clone());
                stage = Stage::Load;
            }
        }
        clk.tick().await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Version B: multiple clk.tick().await in one loop
//
// Each .await is a stage boundary. The Rust compiler inspects which names are
// live on both sides of each .await and stores them as fields in the generated
// Future struct — exactly the semantics of a flip-flop.
//
//   product and c_s are live across the first  .await → Stage 1→2 registers
//   sum     is live across the second .await           → Stage 2→3 register
//
// No State enum. No explicit register declarations beyond the computation.
// The data flow reads top-to-bottom in the order it executes.
// ─────────────────────────────────────────────────────────────────────────────
async fn mac_pipeline(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    b: In<Bits<8>, MainClk>,
    c: In<Bits<8>, MainClk>,
    out: Out<Bits<8>, MainClk>,
) {
    loop {
        // Stage 1 — sample inputs and multiply
        let product = a.read() * b.read();  // ─┐ inferred pipeline registers:
        let c_s     = c.read();             //  ├─ both live across first .await
        clk.tick().await;                   // ─┘ (Stage 1 → Stage 2 boundary)

        // Stage 2 — add c from the same cycle a and b were sampled
        // c_s here is the c value captured in Stage 1, not the current c.
        let sum = product + c_s;            // ─┐ inferred pipeline register:
        clk.tick().await;                   // ─┘ lives across second .await (Stage 2 → Stage 3)

        // Stage 3 — output
        out.write(sum);
        clk.tick().await;
    }
}

fn main() {
    // ── Spawn both modules driven by the same clock ───────────────────────────
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (a_drv,   a_in)   = wire::<Bits<8>, MainClk>(Bits::from_lit::<0>());
    let (b_drv,   b_in)   = wire::<Bits<8>, MainClk>(Bits::from_lit::<0>());
    let (c_drv,   c_in)   = wire::<Bits<8>, MainClk>(Bits::from_lit::<0>());

    let (fsm_out, fsm_obs) = wire::<Bits<8>, MainClk>(Bits::from_lit::<0>());
    let (pip_out, pip_obs) = wire::<Bits<8>, MainClk>(Bits::from_lit::<0>());

    let dh_f = fsm_out.dirty_handle();
    let dh_p = pip_out.dirty_handle();

    exec.spawn_wired(
        mac_fsm(clk.clone(), a_in.clone(), b_in.clone(), c_in.clone(), fsm_out),
        vec![dh_f],
    );
    exec.spawn_wired(
        mac_pipeline(clk.clone(), a_in, b_in, c_in, pip_out),
        vec![dh_p],
    );

    // ── Drive inputs ──────────────────────────────────────────────────────────
    // Stage 1 reads inputs at two different phases depending on iteration:
    //   • Iteration 1: pre-edge of cycle 1 (first ever poll of the future)
    //   • Iterations 2+: post-edge of cycles 3, 6, 9 … (after 3rd await resolves)
    //
    // Stage 3 writes the output at post-edge of cycles 2, 5, 8 …
    // Between Stage 3 writes the output register holds its last value (stale).
    //
    // Input groups, aligned to when Stage 1 reads them:
    //   Group A — set before cycle 1, read pre-edge of cycle 1 → output at cycle 2
    //   Group B — set before cycle 3, read post-edge of cycle 3 → output at cycle 5
    //   Group C — set before cycle 6, read post-edge of cycle 6 → output at cycle 8
    let inputs: &[(u8, u8, u8)] = &[
        (2,  3,  4),  // cycle 1: Group A — Stage 1 reads this pre-edge
        (0,  0,  0),  // cycle 2: Stage 1 not reading (don't care)
        (5,  6,  7),  // cycle 3: Group B — Stage 1 reads this post-edge
        (0,  0,  0),  // cycle 4: don't care
        (0,  0,  0),  // cycle 5: don't care
        (10, 10, 5),  // cycle 6: Group C — Stage 1 reads this post-edge
        (0,  0,  0),  // cycle 7: don't care
        (0,  0,  0),  // cycle 8: don't care
        (0,  0,  0),  // cycle 9: don't care
    ];

    let expected: &[u128] = &[
        0,   // cycle 1: initial (Stage 3 has not run yet)
        10,  // cycle 2: Group A: (2*3)+4 = 10
        10,  // cycle 3: stale (Stage 3 not called this cycle)
        10,  // cycle 4: stale
        37,  // cycle 5: Group B: (5*6)+7 = 37
        37,  // cycle 6: stale
        37,  // cycle 7: stale
        105, // cycle 8: Group C: (10*10)+5 = 105
        105, // cycle 9: stale
    ];

    println!("{:>5}  {:>10}  {:>8}  {:>8}  {:>8}  {}",
             "cycle", "inputs", "fsm_out", "pip_out", "expected", "ok");

    let mut all_pass = true;
    for (&(av, bv, cv), &exp) in inputs.iter().zip(expected.iter()) {
        a_drv.write(Bits::from_u8(av));
        b_drv.write(Bits::from_u8(bv));
        c_drv.write(Bits::from_u8(cv));

        exec.tick_clock(&mut clk);

        let f = fsm_obs.read().as_u128();
        let p = pip_obs.read().as_u128();

        let correct = f == exp && p == exp;
        if !correct { all_pass = false; }

        println!("{:>5}  ({:>2},{:>2},{:>2})  {:>8}  {:>8}  {:>8}  {}",
            clk.cycle(), av, bv, cv, f, p, exp,
            if correct { "✓" } else { "✗" });
    }

    println!("\n{}", if all_pass {
        "✓ FSM and pipeline produce identical outputs."
    } else {
        "✗ Mismatch detected."
    });
    if !all_pass { std::process::exit(1); }
}
