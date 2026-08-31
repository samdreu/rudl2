//! `dual_port_ram`: sim ≡ **transpiled** SystemVerilog — the first design using
//! `Memory` to have any transpiled counterpart at all.
//!
//! ## What this closes
//!
//! Before 2026-08-24 the codegen pipeline was completely blind to `Memory`: the
//! IRs had no memory node, and this example failed at the `let memory = ...`
//! binding with "cannot infer bit width". Every guarantee `Memory` carries —
//! ReadFirst read-during-write, port arbitration, the latency pipelines — was
//! therefore *simulation-only*, and Copper's central claim (one source both
//! simulates and synthesises, provably in agreement) did not extend to any design
//! that used memory. `tests/verilog_fifo_memory_new.rs` anchored to a
//! HAND-WRITTEN Verilog FIFO, not to transpiled output.
//!
//! ## Why this example is the right anchor
//!
//! `examples/memory/dual_port_ram.rs` already checks the simulator against an
//! independent hand-written `examples/memory/sv/dual_port_ram.sv` (a textbook
//! block-RAM: `if (enb) dob <= ram[addrb];`). Adding sim ≡ transpiled-SV here
//! chains the two, so the *generated* SystemVerilog is transitively anchored to
//! a memory neither Copper nor its transpiler wrote.
//!
//! The example file is `include!`d rather than copied, so the two checks cannot
//! drift apart.
//!
//! ## The read-during-write case is the one that matters
//!
//! Cycle 3 below writes and reads address 0x0A on the same edge. ReadFirst says
//! the read sees the OLD contents (0), and the next cycle sees 0x1234. That is
//! the single behaviour where a wrong lowering is invisible in every other cycle,
//! so it is driven deliberately rather than left to chance.

mod common;

use common::EquivalenceTest;

include!("../examples/memory/dual_port_ram.rs");

const SRC: &str = include_str!("../examples/memory/dual_port_ram.rs");

/// Independent reference model: a plain `Vec<u16>` and a registered `dob`.
/// ReadFirst — the capture reads the array before the write commits.
fn ref_step(
    mem: &mut Vec<u16>,
    dob: &mut u16,
    ena: bool,
    enb: bool,
    wea: bool,
    addra: usize,
    addrb: usize,
    dia: u16,
) -> u16 {
    let captured = if enb { mem[addrb] } else { *dob };
    if ena && wea {
        mem[addra] = dia;
    }
    *dob = captured;
    captured
}

#[test]
fn dual_port_ram_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("dual_port_ram", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (ena_drv, ena_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (enb_drv, enb_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (wea_drv, wea_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (addra_drv, addra_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (addrb_drv, addrb_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (dia_drv, dia_in) = wire::<Bits<16>, MainClk>(Bits::zero());
    let (dob_out, dob_obs) = wire::<Bits<16>, MainClk>(Bits::zero());

    let dh: DirtyHandle = dob_out.dirty_handle();
    let reads = vec![
        ena_in.wire_id(),
        enb_in.wire_id(),
        wea_in.wire_id(),
        addra_in.wire_id(),
        addrb_in.wire_id(),
        dia_in.wire_id(),
    ];
    exec.spawn_wired(
        dual_port_ram(
            clk.clone(),
            ena_in,
            enb_in,
            wea_in,
            addra_in,
            addrb_in,
            dia_in,
            dob_out,
        ),
        vec![dh],
        reads,
    );

    // (ena, enb, wea, addra, addrb, dia)
    let cases: &[(bool, bool, bool, usize, usize, u16)] = &[
        (true, false, true, 0x05, 0x00, 0xABCD),  // write 0xABCD @5
        (false, true, false, 0x00, 0x05, 0x0000), // read @5 → 0xABCD
        (false, false, false, 0x00, 0x05, 0x0000), // enb=0 → dob holds
        (true, true, true, 0x0A, 0x0A, 0x1234),   // read-during-write @0x0A → old (0)
        (false, true, false, 0x00, 0x0A, 0x0000), // → 0x1234
        (true, false, true, 0x1F, 0x00, 0xDEAD),  // write 0xDEAD @0x1F
        (false, true, false, 0x00, 0x05, 0x0000), // read a DIFFERENT address → 0xABCD
        (false, true, false, 0x00, 0x1F, 0x0000), // read @0x1F → 0xDEAD
        (true, true, false, 0xFF, 0xFF, 0x5555),  // ena=1 wea=0: no write, read @0xFF → 0
        (true, true, true, 0xFF, 0x1F, 0x5555),   // write @0xFF while reading @0x1F
        (false, true, false, 0x00, 0xFF, 0x0000), // → 0x5555
    ];

    let mut ref_mem = vec![0u16; 256];
    let mut ref_dob = 0u16;

    for &(ena, enb, wea, addra, addrb, dia) in cases {
        ena_drv.write(Logic::from_bool(ena));
        enb_drv.write(Logic::from_bool(enb));
        wea_drv.write(Logic::from_bool(wea));
        addra_drv.write(Bits::from_usize(addra));
        addrb_drv.write(Bits::from_usize(addrb));
        dia_drv.write(Bits::from_u16(dia));

        exec.tick_clock(&mut clk);

        let expected = ref_step(&mut ref_mem, &mut ref_dob, ena, enb, wea, addra, addrb, dia);

        let ena_l = Logic::from_bool(ena);
        let enb_l = Logic::from_bool(enb);
        let wea_l = Logic::from_bool(wea);
        let addra_b = Bits::<8>::from_usize(addra);
        let addrb_b = Bits::<8>::from_usize(addrb);
        let dia_b = Bits::<16>::from_u16(dia);
        let exp_b = Bits::<16>::from_u16(expected);
        let dob_b = dob_obs.read();
        let addra_bits = addra_b.as_array();
        let addrb_bits = addrb_b.as_array();
        let dia_bits = dia_b.as_array();
        let exp_bits = exp_b.as_array();

        eq.record(
            &[
                ("enable_a", std::slice::from_ref(&ena_l)),
                ("enable_b", std::slice::from_ref(&enb_l)),
                ("write_a", std::slice::from_ref(&wea_l)),
                ("addr_a", &addra_bits[..]),
                ("addr_b", &addrb_bits[..]),
                ("data_in_a", &dia_bits[..]),
            ],
            &[("data_out_b", &dob_b.as_array()[..])],
            &[("data_out_b", &exp_bits[..])],
        );
    }

    eq.finish();
}

/// The behavioural check could pass by luck on the sampled cycles. Pin the SHAPE
/// too: a memory must be an array written non-blocking inside `always_ff`, and
/// its read must be a continuous read of that array (which is what makes the
/// capture ReadFirst) — not a registered copy that would lag a cycle.
#[test]
fn memory_lowers_to_an_array_with_a_readfirst_capture() {
    let sv = copper_codegen::transpile_source(
        SRC,
        Some("dual_port_ram"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("dual_port_ram should transpile");

    assert!(
        sv.contains("logic [15:0] memory [0:255];"),
        "the memory must be a packed array of 256 16-bit words, got:\n{sv}"
    );
    assert!(
        sv.contains("memory[memory_wr0_addr] <= memory_wr0_data;"),
        "a write must be a non-blocking assign into the array, got:\n{sv}"
    );
    assert!(
        sv.contains("assign memory_rd0_data = memory[memory_rd0_addr];"),
        "the read must be a continuous read of the array — that is what makes a \
         same-cycle read/write ReadFirst, got:\n{sv}"
    );
}
