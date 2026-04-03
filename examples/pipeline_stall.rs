use copper_core::{Clock, ClockDomain, Bit, Logic};
use copper_sim::{emit, HardwareExecutor, HardwareTest};
use copper_macros::hardware;
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

// 2-stage pipeline with ready/valid handshaking
#[hardware(function_typed)]
async fn pipeline_ready_valid(
    clk: Clock<MainClk>,
    in_valid: Arc<Mutex<Bit>>,
    in_data: Arc<Mutex<u8>>,
    out_ready: Arc<Mutex<Bit>>,
) -> (Bit, Bit, u8) {
    let mut s1_valid = Bit::ZERO;
    let mut s1_data: u8 = 0;
    let mut s2_valid = Bit::ZERO;
    let mut s2_data: u8 = 0;

    loop {
        emit!((
            Bit::from_bool(s1_valid == Bit::ZERO || s2_valid == Bit::ZERO || out_ready.lock().unwrap().0 == Logic::One),
            s2_valid,
            s2_data,
        ));

        clk.tick().await;

        let in_v  = *in_valid.lock().unwrap();
        let in_d  = *in_data.lock().unwrap();
        let out_r = *out_ready.lock().unwrap();

        let s2_accept = s2_valid == Bit::ZERO || out_r == Bit::ONE;
        let s1_accept = s1_valid == Bit::ZERO || s2_accept;

        if s2_accept {
            if s1_valid == Bit::ONE {
                s2_valid = Bit::ONE;
                s2_data  = s1_data.wrapping_add(s1_data);
            } else {
                s2_valid = Bit::ZERO;
            }
        }

        if s1_accept {
            if in_v == Bit::ONE {
                s1_valid = Bit::ONE;
                s1_data  = in_d.wrapping_add(1);
            } else {
                s1_valid = Bit::ZERO;
            }
        }
    }
}

fn u8_to_logic_vec(val: u8) -> Vec<Logic> {
    (0..8)
        .map(|i| if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let in_valid  = Arc::new(Mutex::new(Bit::ZERO));
    let in_data   = Arc::new(Mutex::new(0u8));
    let out_ready = Arc::new(Mutex::new(Bit::ONE));
    let out = exec.spawn_function_typed(
        (Bit::ONE, Bit::ZERO, 0u8),
        pipeline_ready_valid(
            clk.clone(),
            Arc::clone(&in_valid),
            Arc::clone(&in_data),
            Arc::clone(&out_ready),
        ),
    );

    let inputs        = vec![1u8, 2, 3, 4, 5];
    let ready_pattern = vec![
        Logic::One, Logic::Zero, Logic::One, Logic::One, Logic::Zero, Logic::One, Logic::One,
    ];

    let mut test = HardwareTest::new("pipeline_stall")
        .with_verilog("verilog/pipeline_stall.v")
        .with_waveform("waveforms/pipeline_stall.vcd");

    let mut in_idx = 0usize;

    println!("=== Pipeline Ready/Valid ===");
    for &ready in ready_pattern.iter() {
        *out_ready.lock().unwrap() = Bit(ready);

        let (in_ready, _, _) = *out.lock().unwrap();
        let can_send = in_ready == Bit::ONE && in_idx < inputs.len();
        if can_send {
            *in_valid.lock().unwrap() = Bit::ONE;
            *in_data.lock().unwrap()  = inputs[in_idx];
            in_idx += 1;
        } else {
            *in_valid.lock().unwrap() = Bit::ZERO;
        }

        exec.tick_clock(&mut clk);

        let iv          = *in_valid.lock().unwrap();
        let id          = *in_data.lock().unwrap();
        let (ir, ov, od) = *out.lock().unwrap();

        println!(
            "cycle {} in_valid={:?} in_ready={:?} in_data={} out_valid={:?} out_ready={:?} out_data={}",
            clk.cycle(), iv.0, ir.0, id, ov.0, ready, od
        );

        let id_logic  = u8_to_logic_vec(id);
        let od_logic  = u8_to_logic_vec(od);
        test.record_cycle(
            clk.cycle() as usize,
            &[
                ("in_valid",  &[iv.0]),
                ("in_data",   &id_logic),
                ("out_ready", &[ready]),
            ],
            &[
                ("in_ready",  &[ir.0]),
                ("out_valid", &[ov.0]),
                ("out_data",  &od_logic),
            ],
        );
    }

    test.finish().assert_passed();
}
