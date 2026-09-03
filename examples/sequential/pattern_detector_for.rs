// A variable-length pattern detector written as a counted `for` over the pattern.
//
// The pattern arrives on a port (`Bits<N>`) and the loop walks it one bit per
// cycle: the loop counter `i` is live across the tick and becomes a register, and
// `pattern.read()[i]` is a dynamic bit select. A mismatch `break`s out of the `for`
// in the same cycle and the outer loop starts a new attempt on the NEXT input bit
// (the mismatched bit is not re-examined, and there is no overlap handling — a
// deliberately simpler machine than `det_010` in pattern_detector_2.rs). A full
// match drives `out_o` (a `RegOut`, write-before-tick Moore) high for one cycle,
// during which the input is not examined. Reset is sampled only between attempts.
//
// Timing, per attempt started at cycle t: a match of all N bits at cycles
// t..t+N-1 raises the output at cycle t+N; the next attempt begins at t+N+1. A
// mismatch at bit k costs cycles t..t+k; the next attempt begins at t+k+1.
//
// The same module is `tests/fixtures/det_for_dut.rs` (the single source the tests
// and the corpus sweep use — the same arrangement as det_010 and its fixture); this
// file is the runnable demo plus its own self-checks. Written 2026-09-02; it found
// two toolchain bugs on the way (see the fixture's header), both fixed.

use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(sequential)]
async fn det_for<const N: usize>(
    clk: Clock<MainClk>,
    rstn: In<Logic, MainClk>,
    pattern: In<Bits<N>, MainClk>,
    in_i: In<Logic, MainClk>,
    out_o: RegOut<Logic, MainClk>,
) {
    loop {
        out_o.write(Logic::Zero);
        if rstn.read() == Logic::Zero {
            clk.tick().await;
        } else {
            let mut ok = true;
            for i in 0..N {
                if in_i.read() != pattern.read()[i] {
                    ok = false;
                    break;
                }
                clk.tick().await;
            }
            if ok {
                out_o.write(Logic::One);
            }
            // Unconditional: a mismatch that `break`s out of the `for` before its
            // tick must still cost a cycle, or the outer loop would spin with no
            // clock boundary (rejected at compile time since 2026-09-02).
            clk.tick().await;
        }
    }
}

// ── Reference model: the source's control flow, walked by hand ────────────────
//
// `Match(i)`: this cycle compares the input with pattern bit `i`. `Done`: the
// `for` completed; this cycle writes 1 and takes the trailing tick. The observed
// value after the edge is the value written this cycle, or the previous value
// when nothing was written (a RegOut holds). Reset is read only at `Match(0)`,
// exactly as the source reads `rstn` only at the loop top.

#[derive(Clone, Copy, Debug, PartialEq)]
enum St {
    Match(usize),
    Done,
}

struct Model<'a> {
    pattern: &'a [bool],
    st: St,
    out: bool,
}

impl<'a> Model<'a> {
    fn new(pattern: &'a [bool]) -> Self {
        Model { pattern, st: St::Match(0), out: false }
    }

    /// One clock: apply `(rstn, x)`, return the output observed after the edge.
    fn step(&mut self, rstn: bool, x: bool) -> bool {
        let n = self.pattern.len();
        let (st, out) = match self.st {
            St::Match(0) => {
                if !rstn {
                    (St::Match(0), false)
                } else if x == self.pattern[0] {
                    (if n == 1 { St::Done } else { St::Match(1) }, false)
                } else {
                    (St::Match(0), false)
                }
            }
            St::Match(i) => {
                if x == self.pattern[i] {
                    (if i + 1 == n { St::Done } else { St::Match(i + 1) }, self.out)
                } else {
                    (St::Match(0), self.out)
                }
            }
            St::Done => (St::Match(0), true),
        };
        self.st = st;
        self.out = out;
        out
    }
}

/// Simulate `det_for::<N>` on `stream` and return the per-cycle output.
fn simulate<const N: usize>(pattern: Bits<N>, stream: &[(bool, bool)]) -> Vec<bool> {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
    let (pat_drv, pat_in) = wire::<Bits<N>, MainClk>(pattern);
    let (in_drv, in_port) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_drv, out_obs) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
    let dh = out_drv.dirty_handle();
    let reads = vec![rstn_in.wire_id(), pat_in.wire_id(), in_port.wire_id()];
    exec.spawn_wired(det_for::<N>(clk.clone(), rstn_in, pat_in, in_port, out_drv), vec![dh], reads);

    let mut out = Vec::with_capacity(stream.len());
    for &(rstn, x) in stream {
        rstn_drv.write(logic(rstn));
        in_drv.write(logic(x));
        pat_drv.write(pattern);
        exec.tick_clock(&mut clk);
        out.push(out_obs.read() == Logic::One);
    }
    out
}

fn logic(b: bool) -> Logic {
    if b { Logic::One } else { Logic::Zero }
}

fn pattern_bools<const N: usize>(p: Bits<N>) -> Vec<bool> {
    (0..N).map(|i| p.get(i) == Logic::One).collect()
}

/// Every mismatch position, a retry straight after a detection, and a reset both
/// between attempts and in the middle of one (ignored there, by design).
fn coverage_stream_010() -> Vec<(bool, bool)> {
    let mut v = vec![(false, false)]; // reset between attempts
    let bits: &[(u8, u8)] = &[
        (1, 0), (1, 1), (1, 0), // cycles 1-3: match            -> fires at 4
        (1, 1),                 // cycle 4: consumed by the detection cycle
        (1, 1),                 // cycle 5: mismatch at bit 0
        (1, 0), (1, 0),         // cycles 6-7: mismatch at bit 1
        (1, 0), (1, 1), (1, 1), // cycles 8-10: mismatch at bit 2
        (1, 0), (1, 1), (1, 0), // cycles 11-13: match          -> fires at 14
        (1, 0),                 // cycle 14: consumed by the detection cycle
        (1, 0), (0, 1), (1, 0), // cycles 15-17: match with reset asserted MID-attempt
                                //   (ignored: rstn is read only between attempts) -> fires at 18
        (0, 0),                 // cycle 18: consumed by the detection cycle (reset ignored there too)
        (0, 0),                 // cycle 19: reset between attempts
        (1, 0), (1, 1), (1, 0), // cycles 20-22: match          -> fires at 23
        (1, 1),                 // cycle 23: consumed by the detection cycle
        (1, 1),                 // cycle 24: idle
    ];
    v.extend(bits.iter().map(|&(r, x)| (r == 1, x == 1)));
    v
}

fn check<const N: usize>(name: &str, pattern: Bits<N>, stream: &[(bool, bool)], verbose: bool) -> usize {
    let sim = simulate::<N>(pattern, stream);
    let p = pattern_bools(pattern);
    let mut model = Model::new(&p);
    let mut mismatches = 0;
    for (c, &(rstn, x)) in stream.iter().enumerate() {
        let expected = model.step(rstn, x);
        let got = sim[c];
        if verbose {
            println!(
                "{name} cycle {c:2}: rstn={} in={} -> out={}{}",
                rstn as u8, x as u8, got as u8,
                if got { "   <-- detected" } else { "" }
            );
        }
        if got != expected {
            mismatches += 1;
            println!("{name} cycle {c}: simulator {} but the model says {}", got as u8, expected as u8);
        }
    }
    mismatches
}

fn main() {
    // A demo AND a self-check: the simulator against the hand model of the source,
    // on the 010 coverage stream and on a longer pattern (the point of the `for`).
    let m1 = check::<3>("det_for<3> 010 ", Bits::<3>::from_lit::<0b010>(), &coverage_stream_010(), true);
    let m2 = check::<5>("det_for<5> 11010", Bits::<5>::from_lit::<0b01011>(), &stream_11010(), false);
    if m1 + m2 != 0 {
        eprintln!("FAIL: {} cycle(s) disagree with the model", m1 + m2);
        std::process::exit(1);
    }
    println!("det_for: simulator == hand model on both patterns ✓");
}

/// Pattern 1,1,0,1,0 in input order — bit 0 is the FIRST bit compared, so the
/// literal is written least-significant-bit first: 0b01011.
fn stream_11010() -> Vec<(bool, bool)> {
    let mut v = vec![(false, false)];
    let bits: &[u8] = &[
        1, 1, 0, 1, 0, // match
        1, 1, 0, 1, 1, // mismatch at bit 4
        1, 1, 0, 1, 0, // match
        0, 1, 0,       // mismatches at bit 0, bit 2
        1, 1, 0, 1, 0, // match
        1, 1, 1,       // mismatch at bit 2, then bit 0
        1, 1, 0, 1, 0, // match
    ];
    v.extend(bits.iter().map(|&x| (true, x == 1)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn det_for_010_matches_the_hand_model_on_the_coverage_stream() {
        assert_eq!(check::<3>("010", Bits::<3>::from_lit::<0b010>(), &coverage_stream_010(), false), 0);
    }

    #[test]
    fn det_for_010_fires_exactly_where_the_source_says() {
        let sim = simulate::<3>(Bits::<3>::from_lit::<0b010>(), &coverage_stream_010());
        let fired: Vec<usize> = sim.iter().enumerate().filter(|&(_, &v)| v).map(|(c, _)| c).collect();
        // Four detections, each observed the cycle after its last bit — including
        // the one whose attempt had reset asserted in the middle (cycles 15-17).
        assert_eq!(fired, vec![4, 14, 18, 23]);
    }

    #[test]
    fn det_for_5_matches_the_hand_model_on_a_longer_pattern() {
        assert_eq!(check::<5>("11010", Bits::<5>::from_lit::<0b01011>(), &stream_11010(), false), 0);
    }

    #[test]
    fn det_for_1_is_a_one_bit_matcher() {
        // N = 1: every matching bit fires the cycle after, and each attempt costs
        // two cycles (the bit, then the trailing tick), so back-to-back ones fire
        // on alternate cycles.
        let stream: Vec<(bool, bool)> = std::iter::once((false, false))
            .chain([1u8, 1, 1, 1, 0, 1, 0, 0].iter().map(|&x| (true, x == 1)))
            .collect();
        assert_eq!(check::<1>("1", Bits::<1>::from_lit::<1>(), &stream, false), 0);
    }
}
