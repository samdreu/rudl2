//! Preloaded memory (`from_fn` / `from_contents`): sim ≡ transpiled SystemVerilog.
//!
//! Closes the `TODO` P4 item "from_fn / from_contents preload equivalence through
//! the transpiled path", which had been blocked first on memory not transpiling at
//! all and then on choosing an emitted form for the contents.
//!
//! ## The form, and its cost
//!
//! Contents are emitted as an `initial` block — the user's call over `$readmemh`.
//! `initial` is what Verilator executes at time 0 and what FPGA tools read to infer
//! an initialized block RAM, and it keeps the design a single self-contained file.
//! It is **not universally synthesizable** (most ASIC flows ignore `initial`), so a
//! preloaded memory is a simulation-and-FPGA construct by nature. That is a real
//! limitation of the construct, not of this lowering.
//!
//! ## Why the transpiler does not evaluate the preload
//!
//! `from_fn(clk, N, |i| f(i))` is emitted as the fill loop it describes:
//!
//! ```systemverilog
//! initial begin
//!     for (int i = 0; i < 16; i++) begin
//!         rom[i] = 16'(((i * 32'd3) + 32'd7));
//!     end
//! end
//! ```
//!
//! The transpiler does not run Rust, so evaluating the closure was never an
//! option — and it does not need to be. This is also why
//! `examples/cpu/rv32i_cpu.rs` still cannot transpile its `from_contents(clk,
//! flat)`: that `Vec` is built at run time and has no source-level form to emit.
//! The rule is pinned in `copper-codegen/tests/unsupported_constructs.rs`.
//!
//! ## Both DUTs are ROMs on purpose
//!
//! 1 read port, 0 write ports. Nothing but the preload can make the output right,
//! so a missing `initial` block reads as zeros instead of hiding behind a write.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain, Memory};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/preloaded_rom_dut.rs");
const SRC: &str = include_str!("fixtures/preloaded_rom_dut.rs");

#[test]
fn from_fn_preload_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("rom_from_fn", SRC, Some("rom_from_fn"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (a_drv, a_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (d_out, d_obs) = wire::<Bits<16>, MainClk>(Bits::zero());
    let dh = d_out.dirty_handle();
    let reads = vec![a_in.wire_id()];
    exec.spawn_wired(rom_from_fn(clk.clone(), a_in, d_out), vec![dh], reads);

    // Every address, in an order that is not the fill order — a reversed or
    // off-by-one fill cannot survive this.
    for &addr in &[0usize, 15, 1, 14, 7, 8, 2, 13, 3, 12, 4, 11, 5, 10, 6, 9] {
        a_drv.write(Bits::<4>::from_usize(addr));
        exec.tick_clock(&mut clk);

        // Independent reference: the same rule the closure states.
        let expected = Bits::<16>::from_usize(addr * 3 + 7);

        let a_b = Bits::<4>::from_usize(addr);
        let d_b = d_obs.read();
        eq.record(
            &[("addr", &a_b.as_array()[..])],
            &[("data", &d_b.as_array()[..])],
            &[("data", &expected.as_array()[..])],
        );
    }

    eq.finish();
}

#[test]
fn from_contents_preload_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("rom_from_contents", SRC, Some("rom_from_contents"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (a_drv, a_in) = wire::<Bits<2>, MainClk>(Bits::zero());
    let (d_out, d_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = d_out.dirty_handle();
    let reads = vec![a_in.wire_id()];
    exec.spawn_wired(rom_from_contents(clk.clone(), a_in, d_out), vec![dh], reads);

    const WORDS: [u8; 4] = [0xAB, 0x12, 0xF0, 0x34];

    for &addr in &[0usize, 2, 1, 3, 3, 0, 1, 2] {
        a_drv.write(Bits::<2>::from_usize(addr));
        exec.tick_clock(&mut clk);

        let expected = Bits::<8>::from_u8(WORDS[addr]);

        let a_b = Bits::<2>::from_usize(addr);
        let d_b = d_obs.read();
        eq.record(
            &[("addr", &a_b.as_array()[..])],
            &[("data", &d_b.as_array()[..])],
            &[("data", &expected.as_array()[..])],
        );
    }

    eq.finish();
}

/// Shape pins. The behavioural checks above would also pass if the contents
/// arrived some other way, so assert the emitted form directly: a fill LOOP for
/// `from_fn` (not 16 unrolled words — the point is that the description is
/// emitted, not evaluated) and one blocking assign per word for `from_contents`.
#[test]
fn preload_emits_an_initial_block() {
    let fill = copper_codegen::transpile_source(
        SRC,
        Some("rom_from_fn"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("rom_from_fn should transpile");

    assert!(
        fill.contains("initial begin"),
        "a preload must emit an initial block, got:\n{fill}"
    );
    assert!(
        fill.contains("for (int i = 0; i < 16; i++)"),
        "`from_fn` must emit the fill LOOP it describes, not evaluated words, got:\n{fill}"
    );
    assert!(
        fill.contains("rom[i] = 16'("),
        "the fill must be width-cast to the element type (an unresized assignment \
         is a fatal Verilator width warning), got:\n{fill}"
    );

    let words = copper_codegen::transpile_source(
        SRC,
        Some("rom_from_contents"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("rom_from_contents should transpile");

    assert!(
        words.contains("rom[0] = 8'd171;") && words.contains("rom[2] = 8'd240;"),
        "`from_contents` must emit its words in order, got:\n{words}"
    );
}
