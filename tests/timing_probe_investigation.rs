//! TEMP investigation (user: "investigate the +1 first"): does the atomic sim's
//! registered-output timing match hand-written Verilog, and which coding?
//! Compares the atomic sim against INDEPENDENT hand-written .sv (not the transpiler)
//! via the same drive→posedge→sample verilator harness the examples use.
use copper_core::port::{registered_wire, wire, In, Out, RegOut};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};
struct MainClk;
impl ClockDomain for MainClk {}

// D flip-flop: sample d before the tick, drive q after. Hardware: q <= d.
#[hardware(sequential)]
async fn dff(clk: Clock<MainClk>, d: In<Bits<8>, MainClk>, q: Out<Bits<8>, MainClk>) {
    loop {
        let x = d.read();
        clk.tick().await;
        q.write(x);
    }
}

// D flip-flop, the OTHER coding: read d AFTER the tick, then write. Also q <= d.
#[hardware(sequential)]
async fn dff_after(clk: Clock<MainClk>, d: In<Bits<8>, MainClk>, q: Out<Bits<8>, MainClk>) {
    loop {
        clk.tick().await;
        q.write(d.read());
    }
}

// Enable flip-flop (genuinely HELD output): q <= d only when sel; holds otherwise.
#[hardware(sequential)]
async fn enff(
    clk: Clock<MainClk>,
    sel: In<Bits<8>, MainClk>,
    d: In<Bits<8>, MainClk>,
    q: Out<Bits<8>, MainClk>,
) {
    loop {
        clk.tick().await;
        if sel.read() == Bits::from_u8(1) {
            q.write(d.read());
        }
    }
}

// The mac_fsm held-output pattern that justified the atomic migration: 3-state
// FSM (Load->Mul->Out), out.write only in the Out state (held otherwise).
#[derive(Clone, Copy)]
enum Stage { Load, Mul, Out }
#[hardware(sequential)]
async fn mac_fsm(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    b: In<Bits<8>, MainClk>,
    c: In<Bits<8>, MainClk>,
    out: RegOut<Bits<8>, MainClk>,
) {
    let mut stage = Stage::Load;
    let mut product: Bits<8> = Bits::zero();
    let mut c_latch: Bits<8> = Bits::zero();
    let mut result: Bits<8> = Bits::zero();
    loop {
        match stage {
            Stage::Load => { product = a.read() * b.read(); c_latch = c.read(); stage = Stage::Mul; }
            Stage::Mul  => { result = product.clone() + c_latch.clone(); stage = Stage::Out; }
            Stage::Out  => { out.write(result.clone()); stage = Stage::Load; }
        }
        clk.tick().await;
    }
}

// Counter, "write before tick then increment" — the shape the executor unit tests
// re-baselined to [0,1,2].
#[hardware(sequential)]
async fn counter(clk: Clock<MainClk>, q: Out<Bits<8>, MainClk>) {
    let mut v: Bits<8> = Bits::zero();
    loop {
        q.write(v.clone());
        clk.tick().await;
        v = v + Bits::from_u8(1);
    }
}

fn run_dff(sv: &str) -> bool {
    let inputs: Vec<u8> = vec![10, 20, 30, 40, 50, 60];
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (d_drv, d_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (q_drv, q_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = q_drv.dirty_handle();
    let reads = vec![d_in.wire_id()];
    exec.spawn_wired(dff(clk.clone(), d_in, q_drv), vec![dh], reads);
    let mut test = HardwareTest::new("dff").with_verilog(sv);
    let mut sim = Vec::new();
    for &v in &inputs {
        d_drv.write(Bits::from_u8(v));
        exec.tick_clock(&mut clk);
        let q = q_obs.read();
        sim.push(q.as_u8());
        let d_b = Bits::<8>::from_u8(v);
        test.record_cycle(sim.len() - 1, &[("d", &d_b[..])], &[("q", &q[..])]);
    }
    eprintln!("[dff]     sim q trace: {sim:?}   (ref sv: {sv})");
    let r = test.finish();
    matches!(r.verilator_ok, Some(true))
}

fn run_dff_after(sv: &str) -> bool {
    let inputs: Vec<u8> = vec![10, 20, 30, 40, 50, 60];
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (d_drv, d_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (q_drv, q_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = q_drv.dirty_handle();
    let reads = vec![d_in.wire_id()];
    exec.spawn_wired(dff_after(clk.clone(), d_in, q_drv), vec![dh], reads);
    let mut test = HardwareTest::new("dff_after").with_verilog(sv);
    let mut sim = Vec::new();
    for &v in &inputs {
        d_drv.write(Bits::from_u8(v));
        exec.tick_clock(&mut clk);
        let q = q_obs.read();
        sim.push(q.as_u8());
        let d_b = Bits::<8>::from_u8(v);
        test.record_cycle(sim.len() - 1, &[("d", &d_b[..])], &[("q", &q[..])]);
    }
    eprintln!("[dff_after] sim q trace: {sim:?}   (ref sv: {sv})");
    matches!(test.finish().verilator_ok, Some(true))
}

fn run_enff(sv: &str) -> bool {
    // sel pattern: 1,0,1,1,0,1  → q updates on cycles 0,2,3,5; holds on 1,4.
    let sel: Vec<u8> = vec![1, 0, 1, 1, 0, 1];
    let d: Vec<u8> = vec![11, 22, 33, 44, 55, 66];
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (sel_drv, sel_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (d_drv, d_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (q_drv, q_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = q_drv.dirty_handle();
    let reads = vec![sel_in.wire_id(), d_in.wire_id()];
    exec.spawn_wired(enff(clk.clone(), sel_in, d_in, q_drv), vec![dh], reads);
    let mut test = HardwareTest::new("enff").with_verilog(sv);
    let mut sim = Vec::new();
    for i in 0..6usize {
        sel_drv.write(Bits::from_u8(sel[i]));
        d_drv.write(Bits::from_u8(d[i]));
        exec.tick_clock(&mut clk);
        let q = q_obs.read();
        sim.push(q.as_u8());
        let sel_b = Bits::<8>::from_u8(sel[i]);
        let d_b = Bits::<8>::from_u8(d[i]);
        test.record_cycle(i, &[("sel", &sel_b[..]), ("d", &d_b[..])], &[("q", &q[..])]);
    }
    eprintln!("[enff]    sim q trace: {sim:?}   (ref sv: {sv})");
    matches!(test.finish().verilator_ok, Some(true))
}

fn run_mac_fsm(sv: &str) -> bool {
    // Drive one input group (2,3,4)=>10 for cycle 0, then hold zeros. Watch out.
    let a = [2u8, 0, 0, 0, 0, 0, 0, 0, 0];
    let b = [3u8, 0, 0, 0, 0, 0, 0, 0, 0];
    let c = [4u8, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (c_drv, c_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (o_drv, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_drv.dirty_handle();
    let reads = vec![a_in.wire_id(), b_in.wire_id(), c_in.wire_id()];
    exec.spawn_wired(mac_fsm(clk.clone(), a_in, b_in, c_in, o_drv), vec![dh], reads);
    let mut test = HardwareTest::new("mac_fsm").with_verilog(sv);
    let mut sim = Vec::new();
    for i in 0..9usize {
        a_drv.write(Bits::from_u8(a[i]));
        b_drv.write(Bits::from_u8(b[i]));
        c_drv.write(Bits::from_u8(c[i]));
        exec.tick_clock(&mut clk);
        let o = o_obs.read();
        sim.push(o.as_u8());
        let ab = Bits::<8>::from_u8(a[i]);
        let bb = Bits::<8>::from_u8(b[i]);
        let cb = Bits::<8>::from_u8(c[i]);
        test.record_cycle(i, &[("a", &ab[..]), ("b", &bb[..]), ("c", &cb[..])], &[("out", &o[..])]);
    }
    eprintln!("[mac_fsm] sim out trace: {sim:?}   (ref sv: {sv})");
    let r = test.finish();
    for e in &r.errors { eprintln!("    mac_fsm: {e}"); }
    matches!(r.verilator_ok, Some(true))
}

fn run_counter(sv: &str, name: &str) -> bool {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (q_drv, q_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = q_drv.dirty_handle();
    exec.spawn_wired(counter(clk.clone(), q_drv), vec![dh], vec![]);
    let mut test = HardwareTest::new(name).with_verilog(sv);
    let mut sim = Vec::new();
    for i in 0..6usize {
        exec.tick_clock(&mut clk);
        let q = q_obs.read();
        sim.push(q.as_u8());
        test.record_cycle(i, &[], &[("q", &q[..])]);
    }
    eprintln!("[counter] sim q trace: {sim:?}   (ref sv: {sv})");
    let r = test.finish();
    for e in &r.errors { eprintln!("    {name}: {e}"); }
    matches!(r.verilator_ok, Some(true))
}

#[test]
#[ignore = "temp timing investigation — run with --ignored --nocapture"]
fn probe_registered_timing() {
    eprintln!("\n=== DFF read-before-tick (q <= d) ===");
    let dff_ok = run_dff("tests/fixtures/timing_probe_sv/dff.sv");
    eprintln!("=== DFF read-after-tick (q <= d) ===");
    let dff_after_ok = run_dff_after("tests/fixtures/timing_probe_sv/dff.sv");
    eprintln!("=== Enable-FF / held output (if sel: q <= d) ===");
    let enff_ok = run_enff("tests/fixtures/timing_probe_sv/enff.sv");
    eprintln!("=== mac_fsm held output vs faithful hand-written FSM .sv ===");
    let mac_ok = run_mac_fsm("tests/fixtures/timing_probe_sv/mac_fsm.sv");
    eprintln!("=== Counter vs REGISTERED-output .sv (q<=v; v<=v+1) ===");
    let creg = run_counter("tests/fixtures/timing_probe_sv/counter_reg.sv", "counter_reg");
    eprintln!("=== Counter vs COMBINATIONAL-output .sv (assign q=v; v<=v+1) ===");
    let ccomb = run_counter("tests/fixtures/timing_probe_sv/counter_comb.sv", "counter_comb");
    eprintln!("\nRESULT: dff (read-before-tick) matches q<=d ... {dff_ok}");
    eprintln!("RESULT: dff (read-after-tick)  matches q<=d ... {dff_after_ok}");
    eprintln!("RESULT: enable-ff (held) matches if-sel q<=d .. {enff_ok}");
    eprintln!("RESULT: mac_fsm (held) matches faithful FSM sv  {mac_ok}");
    eprintln!("RESULT: counter matches REGISTERED-output ..... {creg}");
    eprintln!("RESULT: counter matches COMBINATIONAL-output .. {ccomb}");
}
