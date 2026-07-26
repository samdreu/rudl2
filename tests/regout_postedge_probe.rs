//! TEMP: does the RegOut primitive compose with the post-edge executor to restore
//! registered (+1) output timing? Compare a plain-Out counter (combinational,
//! [1,2,3]) against a RegOut counter (registered, expect [0,1,2]).
use copper_core::port::{registered_wire, wire, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_sim::{HardwareExecutor, HardwareModule};
struct MainClk;
impl ClockDomain for MainClk {}

#[test]
#[ignore = "temp RegOut/post-edge composition probe"]
fn regout_defers_one_cycle_under_postedge() {
    // Plain-Out counter: write v before tick, increment after.
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (o_drv, o_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = o_drv.dirty_handle();
    let clk_c = clk.clone();
    exec.spawn_wired(
        HardwareModule::__new(async move {
            let mut v: Bits<8> = Bits::zero();
            loop {
                o_drv.write(v.clone());
                clk_c.tick().await;
                v = v + Bits::from_u8(1);
            }
        }),
        vec![dh],
    );
    let mut plain = Vec::new();
    for _ in 0..5 {
        exec.tick_clock(&mut clk);
        plain.push(o_obs.read().as_u8());
    }

    // RegOut counter: identical body, registered output port.
    let mut clk2 = Clock::<MainClk>::new();
    let mut exec2 = HardwareExecutor::new();
    let (r_drv, r_obs) = registered_wire::<Bits<8>, MainClk>(&clk2, Bits::zero());
    let dh2 = r_drv.dirty_handle();
    let clk2_c = clk2.clone();
    exec2.spawn_wired(
        HardwareModule::__new(async move {
            let mut v: Bits<8> = Bits::zero();
            loop {
                r_drv.write(v.clone());
                clk2_c.tick().await;
                v = v + Bits::from_u8(1);
            }
        }),
        vec![dh2],
    );
    let mut reg = Vec::new();
    for _ in 0..5 {
        exec2.tick_clock(&mut clk2);
        reg.push(r_obs.read().as_u8());
    }

    eprintln!("plain Out  counter: {plain:?}   (expect combinational [1,2,3,4,5])");
    eprintln!("RegOut     counter: {reg:?}   (expect registered   [0,1,2,3,4])");
    assert_eq!(plain, vec![1, 2, 3, 4, 5], "plain Out should be combinational under post-edge");
    assert_eq!(reg, vec![0, 1, 2, 3, 4], "RegOut should defer one cycle (registered)");
}
