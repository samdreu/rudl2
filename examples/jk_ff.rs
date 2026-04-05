use copper_core::{Bit, Clock, ClockDomain, Logic};
use copper_sim::{emit, HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// emit-then-tick: state is computed and emitted BEFORE the clock edge
#[copper_macros::hardware]
async fn jk_ff(
    j: Arc<Mutex<Bit>>,
    k: Arc<Mutex<Bit>>,
    clk: Clock<MainClk>,
) -> Bit {
    let mut q = Bit::ZERO;

    loop {
        match (*j.lock().unwrap(), *k.lock().unwrap()) {
            (Bit::ZERO, Bit::ZERO) => {}
            (Bit::ZERO, Bit::ONE)  => { q = Bit::ZERO; }
            (Bit::ONE,  Bit::ZERO) => { q = Bit::ONE; }
            (Bit::ONE,  Bit::ONE)  => {
                q = if q == Bit::ZERO { Bit::ONE } else { Bit::ZERO };
            }
            _ => { q = Bit::X; }
        }
        emit!(q);
        clk.tick().await;

    }
}

// tick-then-emit: state is emitted AFTER the clock edge (registered output)
async fn jk_ff_tick_then_emit(
    j: Arc<Mutex<Bit>>,
    k: Arc<Mutex<Bit>>,
    clk: Clock<MainClk>,
) -> Bit {
    let mut q = Bit::ZERO;

    loop {

        match (*j.lock().unwrap(), *k.lock().unwrap()) {
            (Bit::ZERO, Bit::ZERO) => {}
            (Bit::ZERO, Bit::ONE)  => { q = Bit::ZERO; }
            (Bit::ONE,  Bit::ZERO) => { q = Bit::ONE; }
            (Bit::ONE,  Bit::ONE)  => {
                q = if q == Bit::ZERO { Bit::ONE } else { Bit::ZERO };
            }
            _ => { q = Bit::X; }
        }
        clk.tick().await;
        emit!(q);

    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let j = Arc::new(Mutex::new(Bit::ZERO));
    let k = Arc::new(Mutex::new(Bit::ONE));

    let out_emit_then_tick = exec.spawn_function_typed(
        Bit::ZERO,
        jk_ff(Arc::clone(&j), Arc::clone(&k), clk.clone()),
    );
    let out_tick_then_emit = exec.spawn_function_typed(
        Bit::ZERO,
        jk_ff_tick_then_emit(Arc::clone(&j), Arc::clone(&k), clk.clone()),
    );

    let pattern = [
        (Bit::ZERO, Bit::ONE),
        (Bit::ONE,  Bit::ZERO),
        (Bit::ZERO, Bit::ZERO),
        (Bit::ONE,  Bit::ONE),
        (Bit::ONE,  Bit::ONE),
        (Bit::ZERO, Bit::ONE),
        (Bit::ZERO, Bit::ZERO),
        (Bit::ONE,  Bit::ZERO),
    ];

    println!("JK FF ordering comparison (pre/post sampling)");
    println!("cyc | j k | emit_then_tick(pre post) | tick_then_emit(pre post) | diff");
    println!("----+-----+--------------------------+--------------------------+------");

    // Two separate tests:
    //   - emit_then_tick: combinational semantics (output before clock edge)
    //   - tick_then_emit: registered semantics (output after clock edge, matches Verilator)
    //
    // record_cycle_phased captures both pre and post for the phased VCD, and uses
    // post-clock values for the Verilator cross-validation trace.
    // Note: jk_ff uses tuple-pattern match `(j, k)` which is not yet supported
    // by the sequential Verilog codegen (requires FSM expansion for multi-variable
    // patterns). Using hand-written Verilog for cross-validation.
    let verilog_path = "verilog/jk_ff.v";

    let mut test_registered = HardwareTest::new("jk_ff")
        .with_verilog(verilog_path)
        .with_waveform("waveforms/jk_ff.vcd")
        .with_phased_waveform("waveforms/jk_ff_phased.vcd")
        .with_verilator_waveform("waveforms/jk_ff_verilator.vcd");

    for (cycle, (jv, kv)) in pattern.iter().enumerate() {
        *j.lock().unwrap() = *jv;
        *k.lock().unwrap() = *kv;

        // Combinational settle (pre-clock edge)
        exec.poll_tasks();
        let q_a_pre  = *out_emit_then_tick.lock().unwrap();
        let q_b_pre  = *out_tick_then_emit.lock().unwrap();

        // Clock edge
        exec.advance(&mut clk);

        // Registered settle (post-clock edge)
        exec.poll_tasks();
        let q_a_post = *out_emit_then_tick.lock().unwrap();
        let q_b_post = *out_tick_then_emit.lock().unwrap();

        let diff = if q_a_pre != q_b_pre || q_a_post != q_b_post { "YES" } else { "NO" };
        println!(
            " {:>2} | {} {} |          {}   {}           |          {}   {}           | {}",
            cycle,
            if *jv == Bit::ONE { 1 } else { 0 },
            if *kv == Bit::ONE { 1 } else { 0 },
            if q_a_pre  == Bit::ONE { 1 } else { 0 },
            if q_a_post == Bit::ONE { 1 } else { 0 },
            if q_b_pre  == Bit::ONE { 1 } else { 0 },
            if q_b_post == Bit::ONE { 1 } else { 0 },
            diff
        );

        // record_cycle_phased:
        //   inputs        = j, k (same before and after clock edge)
        //   pre_outputs   = emit_then_tick q BEFORE edge (combinational output)
        //   post_outputs  = emit_then_tick q AFTER edge (stable combinational output)
        //
        // The phased VCD will show q_a_pre at clk=0 and q_a_post at clk=1.
        test_registered.record_cycle_phased(
            cycle,
            &[("j", &[jv.0]), ("k", &[kv.0])],
            &[("q", &[q_a_pre.0])],   // pre-clock: combinational output
            &[("q", &[q_a_post.0])],  // post-clock: combinational output
        );
    }

    // Expected: emit-then-tick JK FF behavior (combinational, starts at q=0)
    // (J,K) -> Q after clock edge:
    // (0,1)->0  (1,0)->1  (0,0)->1  (1,1)->0  (1,1)->1  (0,1)->0  (0,0)->0  (1,0)->1
    let expected = SimulationTrace::from_cycles(vec![
        make_cycle(0, &[("j", &[Logic::Zero]), ("k", &[Logic::One])],  &[("q", &[Logic::Zero])]),
        make_cycle(1, &[("j", &[Logic::One]),  ("k", &[Logic::Zero])], &[("q", &[Logic::One])]),
        make_cycle(2, &[("j", &[Logic::Zero]), ("k", &[Logic::Zero])], &[("q", &[Logic::One])]),
        make_cycle(3, &[("j", &[Logic::One]),  ("k", &[Logic::One])],  &[("q", &[Logic::Zero])]),
        make_cycle(4, &[("j", &[Logic::One]),  ("k", &[Logic::One])],  &[("q", &[Logic::One])]),
        make_cycle(5, &[("j", &[Logic::Zero]), ("k", &[Logic::One])],  &[("q", &[Logic::Zero])]),
        make_cycle(6, &[("j", &[Logic::Zero]), ("k", &[Logic::Zero])], &[("q", &[Logic::Zero])]),
        make_cycle(7, &[("j", &[Logic::One]),  ("k", &[Logic::Zero])], &[("q", &[Logic::One])]),
    ]);

    test_registered.finish_with_expected(&expected).assert_passed();
}
