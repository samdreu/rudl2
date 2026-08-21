//! TEMP: nail the memory read latency (user: "nail the memory latency first").
//! The Memory primitive is a faithful 1-cycle read. Question: does the example's
//! +1 come from the DUT straddling the tick (issue read pre-tick, drive dob
//! post-tick)? Compare two DUT structures against the SAME 1-cycle block-RAM .sv,
//! both preloaded with ram[i] = i + 100.
use copper_core::port::{wire, In, Out};
use copper_core::types::{Bits, Logic};
use copper_core::{Clock, ClockDomain, Memory};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareTest};
struct MainClk;
impl ClockDomain for MainClk {}

fn preloaded(clk: Clock<MainClk>) -> Memory<Bits<16>, 1, 1, MainClk, 1, 1> {
    Memory::from_fn(clk, 256, |i| Bits::from_u16((i as u16) + 100))
}

// STRADDLE (current example shape): issue read pre-tick, consume + drive dob
// post-tick. Suspected 2-cycle-index under atomic.
#[hardware(sequential)]
async fn ram_straddle(
    clk: Clock<MainClk>,
    enb: In<Logic, MainClk>,
    addrb: In<Bits<8>, MainClk>,
    dob: Out<Bits<16>, MainClk>,
) {
    let memory = preloaded(clk.clone());
    let mut data: Bits<16> = Bits::zero();
    loop {
        if enb.read() == Logic::One {
            memory.read_port::<0>().read(addrb.read().as_usize());
        }
        clk.tick().await;
        if memory.read_port::<0>().is_ready() {
            data = memory.read_port::<0>().data();
        }
        dob.write(data.clone());
    }
}

// OUTPUT-BEFORE-TICK: drive dob (from the previous read) BEFORE the tick, then
// issue the next read. dob is a pre-tick write (like counter/mac_fsm → observed
// same cycle). Hypothesis: this collapses to 1-cycle, matching the .sv.
#[hardware(sequential, allow_pretick_alignment)]
async fn ram_prewrite(
    clk: Clock<MainClk>,
    enb: In<Logic, MainClk>,
    addrb: In<Bits<8>, MainClk>,
    dob: Out<Bits<16>, MainClk>,
) {
    let memory = preloaded(clk.clone());
    let mut data: Bits<16> = Bits::zero();
    loop {
        if memory.read_port::<0>().is_ready() {
            data = memory.read_port::<0>().data();
        }
        dob.write(data.clone());
        if enb.read() == Logic::One {
            memory.read_port::<0>().read(addrb.read().as_usize());
        }
        clk.tick().await;
    }
}

#[test]
#[ignore = "temp mem-latency probe"]
fn probe_mem_latency() {
    for (name, straddle) in [("straddle", true), ("prewrite", false)] {
        let addrs: Vec<u8> = vec![5, 6, 7, 8, 9, 10];
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();
        let (enb_drv, enb_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (ad_drv, ad_in) = wire::<Bits<8>, MainClk>(Bits::zero());
        let (o_drv, o_obs) = wire::<Bits<16>, MainClk>(Bits::zero());
        let dh = o_drv.dirty_handle();
        let reads = vec![enb_in.wire_id(), ad_in.wire_id()];
        if straddle {
            exec.spawn_wired(ram_straddle(clk.clone(), enb_in, ad_in, o_drv), vec![dh], reads);
        } else {
            exec.spawn_wired(ram_prewrite(clk.clone(), enb_in, ad_in, o_drv), vec![dh], reads);
        }
        let mut test = HardwareTest::new("ram1").with_verilog("tests/fixtures/timing_probe_sv/ram1.sv");
        let mut sim = Vec::new();
        for i in 0..6usize {
            enb_drv.write(Logic::One);
            ad_drv.write(Bits::from_u8(addrs[i]));
            exec.tick_clock(&mut clk);
            let o = o_obs.read();
            sim.push(o.as_u16());
            let e = Logic::One;
            let ab = Bits::<8>::from_u8(addrs[i]);
            test.record_cycle(
                i,
                &[("enb", std::slice::from_ref(&e)), ("addrb", &ab[..])],
                &[("dob", &o[..])],
            );
        }
        let r = test.finish();
        eprintln!("[{name}] sim dob: {sim:?}  verilator_ok={:?}", r.verilator_ok);
        for e in &r.errors {
            eprintln!("    {name}: {e}");
        }
    }
    eprintln!("expected 1-cycle .sv dob (ram[addr]=addr+100): [105,106,107,108,109,110]");
}
