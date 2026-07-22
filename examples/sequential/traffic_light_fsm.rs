use copper_core::types::{Logic, Clock, ClockDomain};
use copper_core::port::{In, Out, wire};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

// Moore FSM: outputs depend only on the current state (phase), not on inputs.
// A high `request` pulse in Green triggers the full cycle:
//   Green → Yellow (2 cycles) → Red (4 cycles) → RedYellow (1 cycle) → Green
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Green,
    Yellow,
    Red,
    RedYellow,
}

#[hardware(sequential)]
async fn traffic_light(
    clk: Clock<MainClk>,
    request: In<Logic, MainClk>,
    red: Out<Logic, MainClk>,
    yellow: Out<Logic, MainClk>,
    green_out: Out<Logic, MainClk>,
) {
    // Both variables live across .await → both become registers in the
    // generated Future struct, i.e. flip-flops in synthesized hardware.
    let mut phase = Phase::Green;
    let mut timer: u8 = 0;

    loop {
        // Pre-edge: drive outputs from current phase.
        // Because this is a Moore FSM, there is no input term here.
        match phase {
            Phase::Green => {
                red.write(Logic::Zero);
                yellow.write(Logic::Zero);
                green_out.write(Logic::One);
            }
            Phase::Yellow => {
                red.write(Logic::Zero);
                yellow.write(Logic::One);
                green_out.write(Logic::Zero);
            }
            Phase::Red => {
                red.write(Logic::One);
                yellow.write(Logic::Zero);
                green_out.write(Logic::Zero);
            }
            Phase::RedYellow => {
                red.write(Logic::One);
                yellow.write(Logic::One);
                green_out.write(Logic::Zero);
            }
        }

        clk.tick().await;

        // Post-edge: sample inputs and compute next state.
        // `request.read()` here is the value stable at the rising edge —
        // equivalent to how a flip-flop samples its D input.
        (phase, timer) = match (phase, timer, request.read()) {
            // Green: hold until a request arrives
            (Phase::Green, _, Logic::One) => (Phase::Yellow,    0),
            (Phase::Green, _, _)          => (Phase::Green,     0),
            // Yellow: 2-cycle dwell (timer 0 → 1 → transition)
            (Phase::Yellow, t, _) if t < 1 => (Phase::Yellow,  t + 1),
            (Phase::Yellow, _, _)           => (Phase::Red,     0),
            // Red: 4-cycle dwell (timer 0 → 1 → 2 → 3 → transition)
            (Phase::Red, t, _) if t < 3    => (Phase::Red,       t + 1),
            (Phase::Red, _, _)             => (Phase::RedYellow, 0),
            // RedYellow: single-cycle, then back to Green
            (Phase::RedYellow, _, _)       => (Phase::Green,     0),
        };
    }
}

fn phase_label(r: Logic, y: Logic, g: Logic) -> &'static str {
    match (r, y, g) {
        (Logic::Zero, Logic::Zero, Logic::One)  => "GREEN",
        (Logic::Zero, Logic::One,  Logic::Zero) => "YELLOW",
        (Logic::One,  Logic::Zero, Logic::Zero) => "RED",
        (Logic::One,  Logic::One,  Logic::Zero) => "RED+YELLOW",
        _                                       => "UNKNOWN",
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (req_drv, req_in)        = wire::<Logic, MainClk>(Logic::Zero);
    let (red_out, red_obs)       = wire::<Logic, MainClk>(Logic::Zero);
    let (yellow_out, yellow_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let (green_out, green_obs)   = wire::<Logic, MainClk>(Logic::Zero);

    // dirty_handle() must be called on Out<T> before it is moved into the module
    let dh_r = red_out.dirty_handle();
    let dh_y = yellow_out.dirty_handle();
    let dh_g = green_out.dirty_handle();

    exec.spawn_wired(
        traffic_light(clk.clone(), req_in, red_out, yellow_out, green_out),
        vec![dh_r, dh_y, dh_g],
    );

    // request goes high on cycle 3 to trigger the phase sequence.
    // The module writes outputs post-edge with the already-updated state, so
    // the new phase is visible in the same cycle the input is sampled —
    // equivalent to a registered FSM where the output and state registers
    // both update on the same clock edge.
    let inputs: &[Logic] = &[
        Logic::Zero, // 1:  Green, no request
        Logic::Zero, // 2:  Green, no request
        Logic::One,  // 3:  request=1 sampled → Yellow visible this cycle
        Logic::Zero, // 4:  Yellow, timer 0→1
        Logic::Zero, // 5:  timer=1 expires → Red visible this cycle
        Logic::Zero, // 6:  Red, timer 0→1
        Logic::Zero, // 7:  Red, timer 1→2
        Logic::Zero, // 8:  Red, timer 2→3
        Logic::Zero, // 9:  timer=3 expires → RedYellow visible this cycle
        Logic::Zero, // 10: RedYellow → Green visible this cycle
        Logic::Zero, // 11: Green, steady state
    ];

    let expected_phases = &[
        "GREEN",
        "GREEN",
        "YELLOW",
        "YELLOW",
        "RED",
        "RED",
        "RED",
        "RED",
        "RED+YELLOW",
        "GREEN",
        "GREEN",
    ];

    println!("{:>5}  {:>7}  {:<12}  {}", "cycle", "request", "phase", "pass");

    let mut all_pass = true;
    for (&req, &expected) in inputs.iter().zip(expected_phases.iter()) {
        req_drv.write(req);
        exec.tick_clock(&mut clk);

        let phase = phase_label(red_obs.read(), yellow_obs.read(), green_obs.read());
        let pass = phase == expected;
        if !pass { all_pass = false; }

        println!(
            "{:>5}  {:>7}  {:<12}  {}",
            clk.cycle(),
            if req == Logic::One { "1" } else { "0" },
            phase,
            if pass { "✓" } else { "✗" },
        );
    }

    println!("\n{}", if all_pass { "✓ All cycles correct." } else { "✗ Mismatch detected." });
    if !all_pass { std::process::exit(1); }
}
