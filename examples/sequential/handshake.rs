// A request/acknowledge handshake counter, written as the sequence of waits it is:
// wait for `req`, take one cycle, wait for `ack`, count the completed handshake.
//
// Each `while` is a state with a self-loop and each `clk.tick().await` is a clock
// edge, so the control flow IS the state machine an HDL would make the designer
// enumerate: four ticks, four states (wait for `req`, the cycle after it, wait for
// `ack`, count), and the transpiler emits exactly that `pc`. `n` is live across
// the ticks and becomes a register; `done` is a `RegOut` because it is written
// before a tick and must hold for the cycles in between.
//
// Timing: `done` shows the count of completed handshakes and updates the cycle
// after the loop returns to its top, which is the cycle after `ack` was seen.
// `req` is sampled every cycle until seen; `ack` likewise, starting the cycle
// after `req` was seen (the middle tick).
//
// The same module is `tests/fixtures/handshake_dut.rs` (the single source the tests
// and the corpus sweep use); the `loop { if … { break } tick }` spelling of the same
// machine is `handshake` in `tests/fixtures/wait_loop_dut.rs`, and
// `tests/handshake_equivalence.rs` checks the two agree cycle for cycle.

use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(sequential)]
async fn handshake(
    clk: Clock<MainClk>,
    req: In<Logic, MainClk>,
    ack: In<Logic, MainClk>,
    done: RegOut<Bits<8>, MainClk>,
) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        done.write(n);
        while req.read() == Logic::Zero {
            clk.tick().await;
        }
        clk.tick().await;
        while ack.read() == Logic::Zero {
            clk.tick().await;
        }
        n = n + Bits::from_lit::<1>();
        clk.tick().await;
    }
}

// ── Reference model: the source's control flow, walked by hand ────────────────
//
// `Top`: publish `done = n`, then test `req`. `WaitReq`: test `req` again (the
// `while` self-loop). `WaitAck`: test `ack`; on success count and return to `Top`.
// The observed `done` after the edge is the value written this cycle (only at
// `Top`) or the previous value (a RegOut holds).

#[derive(Clone, Copy, PartialEq, Debug)]
enum St {
    Top,
    WaitReq,
    WaitAck,
}

struct Model {
    st: St,
    n: u8,
    done: u8,
}

impl Model {
    fn new() -> Self {
        Model { st: St::Top, n: 0, done: 0 }
    }

    /// One clock: apply `(req, ack)`, return `done` as observed after the edge.
    fn step(&mut self, req: bool, ack: bool) -> u8 {
        match self.st {
            St::Top => {
                self.done = self.n;
                self.st = if req { St::WaitAck } else { St::WaitReq };
            }
            St::WaitReq => {
                if req {
                    self.st = St::WaitAck;
                }
            }
            St::WaitAck => {
                if ack {
                    self.n = self.n.wrapping_add(1);
                    self.st = St::Top;
                }
            }
        }
        self.done
    }
}

fn logic(b: bool) -> Logic {
    if b { Logic::One } else { Logic::Zero }
}

/// Simulate on `stream` (one `(req, ack)` per cycle) and return `done` per cycle.
fn simulate(stream: &[(bool, bool)]) -> Vec<u8> {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (req_drv, req_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (ack_drv, ack_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (done_drv, done_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = done_drv.dirty_handle();
    let reads = vec![req_in.wire_id(), ack_in.wire_id()];
    exec.spawn_wired(handshake(clk.clone(), req_in, ack_in, done_drv), vec![dh], reads);

    let mut out = Vec::with_capacity(stream.len());
    for &(req, ack) in stream {
        req_drv.write(logic(req));
        ack_drv.write(logic(ack));
        exec.tick_clock(&mut clk);
        out.push(done_obs.read().as_usize() as u8);
    }
    out
}

/// Every wait length from zero up, `ack` already high when `req` arrives, `ack`
/// arriving late, and back-to-back handshakes with `req` still high.
fn coverage_stream() -> Vec<(bool, bool)> {
    let s: &[(u8, u8)] = &[
        (0, 0), (0, 0),         // idle: waiting for req
        (1, 0),                 // req seen at Top
        (0, 0), (0, 0),         // waiting for ack
        (0, 1),                 // ack seen                       -> done = 1 next cycle
        (1, 1),                 // Top: req already high -> straight to WaitAck
        (1, 1),                 // ack already high: seen at once -> done = 2
        (1, 1),                 // Top again (req still high), and so on
        (0, 0),                 // Top, idle
        (0, 0), (1, 0),         // req after one wait cycle
        (1, 0), (1, 0), (1, 0), (1, 1), // ack after three wait cycles -> done = 3
        (1, 0),                 // Top with req still high (back-to-back)
        (0, 0), (0, 1),         // ack                             -> done = 4
        (0, 0), (0, 0),         // idle
    ];
    s.iter().map(|&(r, a)| (r == 1, a == 1)).collect()
}

fn check(stream: &[(bool, bool)], verbose: bool) -> usize {
    let sim = simulate(stream);
    let mut model = Model::new();
    let mut mismatches = 0;
    for (c, &(req, ack)) in stream.iter().enumerate() {
        let expected = model.step(req, ack);
        if verbose {
            println!("cycle {c:2}: req={} ack={} -> done={}", req as u8, ack as u8, sim[c]);
        }
        if sim[c] != expected {
            mismatches += 1;
            println!("cycle {c}: simulator done={} but the model says {expected}", sim[c]);
        }
    }
    mismatches
}

fn main() {
    // A demo AND a self-check: the simulator against the hand model of the source.
    let m = check(&coverage_stream(), true);
    if m != 0 {
        eprintln!("FAIL: {m} cycle(s) disagree with the model");
        std::process::exit(1);
    }
    println!("handshake: simulator == hand model ✓");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_matches_the_hand_model_on_the_coverage_stream() {
        assert_eq!(check(&coverage_stream(), false), 0);
    }

    #[test]
    fn done_counts_completed_handshakes_one_cycle_after_ack() {
        let sim = simulate(&coverage_stream());
        // `done` steps 0→1→2→3→4 on the cycle after each ack is seen (cycles 5,
        // 7, 15, 18) — i.e. at cycles 6, 8, 16, 19 — and holds in between.
        let steps: Vec<(usize, u8)> = sim
            .iter()
            .enumerate()
            .filter(|&(c, &v)| c > 0 && v != sim[c - 1])
            .map(|(c, &v)| (c, v))
            .collect();
        assert_eq!(steps, vec![(6, 1), (8, 2), (16, 3), (19, 4)]);
    }

    #[test]
    fn a_request_never_acknowledged_never_counts() {
        let stream: Vec<(bool, bool)> = std::iter::repeat((true, false)).take(40).collect();
        assert_eq!(simulate(&stream).last(), Some(&0));
    }
}
