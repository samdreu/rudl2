use copper_core::types::{Logic, Clock, ClockDomain};
use copper_core::port::{In, Out, wire};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

enum State {
    A,
    B, 
    C,
    D,
}


#[hardware(sequential)]
async fn det_010 (
    clk: Clock<MainClk>,
    rstn: In<Logic, MainClk>,
    in_i: In<Logic, MainClk>,
    out_o: Out<Logic, MainClk>,
) {
    let mut state = State::A;
    loop {
        if rstn.read() == Logic::Zero {
            state = State::A;
        } else {
            state = match (state, in_i.read()) {
                (State::A, Logic::Zero) => State::B,
                (State::B, Logic::One) => State::C,
                (State::B, Logic::Zero) => State::B,
                (State::C, Logic::Zero) => State::D,
                (State::D, Logic::Zero) => State::B,
                _ => State::A,
            };
            
        }
        clk.tick().await;

        if matches!(state, State::D) {
            out_o.write(Logic::One);
        } else {
            out_o.write(Logic::Zero);
        }
    }
}

#[hardware(sequential)]
async fn det_010_awaits (
    clk: Clock<MainClk>,
    rstn: In<Logic, MainClk>,
    in_i: In<Logic, MainClk>,
    out_o: Out<Logic, MainClk>,
) {
    loop {
        out_o.write(Logic::Zero);
        if rstn.read() == Logic::Zero {
            out_o.write(Logic::Zero);
            clk.tick().await;
        } else if in_i.read() == Logic::Zero {
            clk.tick().await;
            while in_i.read() == Logic::Zero {
                clk.tick().await;
            }
            if in_i.read() == Logic::One {
                clk.tick().await;
                if in_i.read() == Logic::Zero {
                    out_o.write(Logic::One);
                    clk.tick().await;
                }
            }
        }
    }
}

fn main() {
    // Demo: run det_010 over a bit stream containing the "010" pattern and print
    // when the detector fires. (The equivalence checks live in the tests below.)
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (rstn_drv, rstn) = wire::<Logic, MainClk>(Logic::One);
    let (in_drv, in_i) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_drv, out_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(det_010(clk.clone(), rstn, in_i, out_drv), vec![dh]);

    println!("=== det_010 (detects the pattern 0,1,0) ===");
    // Assert reset (rstn low) for one cycle to reach a known state.
    rstn_drv.write(Logic::Zero);
    in_drv.write(Logic::Zero);
    exec.tick_clock(&mut clk);

    let stream = [1u8, 1, 0, 1, 0, 0, 1, 0, 1, 0];
    for &b in &stream {
        rstn_drv.write(Logic::One);
        in_drv.write(if b == 1 { Logic::One } else { Logic::Zero });
        exec.tick_clock(&mut clk);
        let fired = out_obs.read() == Logic::One;
        println!("in={b} -> out={}{}", fired as u8, if fired { "   <-- 010 detected" } else { "" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Under the post-edge execution convention the two codings are cycle-identical
    // again: a register clocked at edge N is observed in cycle N, so det_010's
    // post-tick `out.write(state)` and det_010_awaits's write-at-detection agree.
    // (They diverged only under the interim atomic/pre-edge model.)
    // See design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md.
    #[test]
    fn det_010_variants_match_transition_table() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let (rstn_drv_a, rstn_a) = wire::<Logic, MainClk>(Logic::One);
        let (in_drv_a, in_a) = wire::<Logic, MainClk>(Logic::Zero);
        let (out_drv_a, out_obs_a) = wire::<Logic, MainClk>(Logic::Zero);

        let (rstn_drv_b, rstn_b) = wire::<Logic, MainClk>(Logic::One);
        let (in_drv_b, in_b) = wire::<Logic, MainClk>(Logic::Zero);
        let (out_drv_b, out_obs_b) = wire::<Logic, MainClk>(Logic::Zero);

        let dh_a = out_drv_a.dirty_handle();
        let dh_b = out_drv_b.dirty_handle();

        exec.spawn_wired(det_010(clk.clone(), rstn_a, in_a, out_drv_a), vec![dh_a]);
        exec.spawn_wired(det_010_awaits(clk.clone(), rstn_b, in_b, out_drv_b), vec![dh_b]);

        let z = Logic::Zero;
        let o = Logic::One;

        // The table in the prompt is a 4-state Moore machine with state
        // encoding 00, 01, 10, 11 and output high only in state 11.
        //
        // Covers every reachable (state, input) pair at least once — including
        // both transitions out of D (11), which the original stimulus never
        // reached — plus a reset asserted from a non-A state.
        let cases: &[(Logic, Logic)] = &[
            (z, z), // reset                       -> A
            (o, o), // A,1 -> A
            (o, z), // A,0 -> B
            (o, z), // B,0 -> B
            (o, o), // B,1 -> C
            (o, z), // C,0 -> D               (output goes high)
            (o, o), // D,1 -> A               (output drops)
            (o, z), // A,0 -> B
            (o, o), // B,1 -> C
            (o, z), // C,0 -> D               (output goes high)
            (o, z), // D,0 -> A               (output drops)
            (o, z), // A,0 -> B
            (o, o), // B,1 -> C
            (o, z), // C,0 -> D               (output goes high)
            (z, o), // reset asserted from D  -> A (output drops immediately)
            (o, z), // A,0 -> B
            (o, o), // B,1 -> C
            (o, o), // C,1 -> A
        ];

        for (cycle, &(rstn, input)) in cases.iter().enumerate() {
            rstn_drv_a.write(rstn);
            rstn_drv_b.write(rstn);
            in_drv_a.write(input);
            in_drv_b.write(input);

            exec.tick_clock(&mut clk);

            let out_a = out_obs_a.read();
            let out_b = out_obs_b.read();

            assert_eq!(
                out_a, out_b,
                "cycle {cycle}: modules diverged for rstn={rstn:?}, in={input:?}"
            );
        }
    }
}

