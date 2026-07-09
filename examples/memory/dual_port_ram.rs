// Single dual-port RAM with single clock

use copper_core::{Bits, Clock, ClockDomain, Logic, Memory, port::{DirtyHandle, In, Out, wire}};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};


struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(sequential)]
async fn dual_port_ram (
    clk: Clock<MainClk>,
    enable_a: In<Logic>,
    enable_b: In<Logic>,
    write_a: In<Logic>,
    addr_a: In<Bits<8>>,
    addr_b: In<Bits<8>>,
    data_in_a: In<Bits<16>>,
    data_out_a: Out<Bits<16>>,
    data_out_b: Out<Bits<16>>
) {
    let memory = Memory::<Bits<16>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 256);
    let mut data: Bits<16> = Bits::zero();

    loop {
        // Pre-edge: stage memory ops with current inputs (processed by advance())
        if enable_a.read() == Logic::One && write_a.read() == Logic::One {
            memory.write_port::<0>().write(addr_a.read().as_usize(), data_in_a.read());
        }
        if enable_b.read() == Logic::One {
            memory.read_port::<0>().read(addr_b.read().as_usize());
        }

        clk.tick().await;

        // Post-edge: capture pipeline results and drive outputs
        if memory.read_port::<0>().is_ready() {
            data = memory.read_port::<0>().data();
        }
        data_out_b.write(data);
    }
}

fn main() {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (ena_drv,   ena_in)    = wire::<Logic,    ()>(Logic::Zero);
    let (enb_drv,   enb_in)    = wire::<Logic,    ()>(Logic::Zero);
    let (wea_drv,   wea_in)    = wire::<Logic,    ()>(Logic::Zero);
    let (addra_drv, addra_in)  = wire::<Bits<8>,  ()>(Bits::zero());
    let (addrb_drv, addrb_in)  = wire::<Bits<8>,  ()>(Bits::zero());
    let (dia_drv,   dia_in)    = wire::<Bits<16>, ()>(Bits::zero());
    let (doa_out,   _doa_obs)  = wire::<Bits<16>, ()>(Bits::zero());
    let (dob_out,   dob_obs)   = wire::<Bits<16>, ()>(Bits::zero());

    let dh: DirtyHandle = dob_out.dirty_handle();
    exec.spawn_wired(
        dual_port_ram(clk.clone(), ena_in, enb_in, wea_in, addra_in, addrb_in, dia_in, doa_out, dob_out),
        vec![dh],
    );

    let mut test = HardwareTest::new("dual_port_ram")
        .with_verilog("examples/memory/sv/dual_port_ram.v")
        .with_waveform("waveforms/dual_port_ram.vcd")
        .with_verilator_waveform("waveforms/dual_port_ram_v.vcd");

    // Reference model — plain Vec<u16> for memory, u16 for the registered dob output.
    // ReadFirst semantics: read captures the old value before the write commits.
    fn ref_step(
        mem: &mut Vec<u16>,
        dob: &mut u16,
        ena: bool, enb: bool, wea: bool,
        addra: usize, addrb: usize,
        dia: u16,
    ) -> u16 {
        let new_dob = if enb { mem[addrb] } else { *dob };
        if ena && wea { mem[addra] = dia; }
        *dob = new_dob;
        new_dob
    }

    // (ena, enb, wea, addra, addrb, dia)
    let cases: &[(bool, bool, bool, usize, usize, u16)] = &[
        (true,  false, true,  0x05, 0x00, 0xABCD), // write 0xABCD to addr 5; enb=0 so dob holds 0
        (false, true,  false, 0x00, 0x05, 0x0000), // read addr 5 → dob = 0xABCD
        (false, false, false, 0x00, 0x05, 0x0000), // enb=0: dob holds previous value
        (true,  true,  true,  0x0A, 0x0A, 0x1234), // simultaneous write+read at addr A (ReadFirst → dob = 0)
        (false, true,  false, 0x00, 0x0A, 0x0000), // read addr A → dob = 0x1234
        // Adversarial: output changes then immediately reads a DIFFERENT address.
        // With delta_yield this would output stale data (re-read addr A); with
        // pre_edge_barrier it correctly reads addr 5.
        (true,  false, true,  0x1F, 0x00, 0xDEAD), // write 0xDEAD to addr 0x1F; enb=0
        (false, true,  false, 0x00, 0x05, 0x0000), // read addr 5 (different from last read addr A) → 0xABCD
    ];

    let mut ref_mem  = vec![0u16; 256];
    let mut ref_dob  = 0u16;
    let mut expected_cycles = Vec::new();

    for (i, &(ena, enb, wea, addra, addrb, dia_val)) in cases.iter().enumerate() {
        ena_drv.write(Logic::from_bool(ena));
        enb_drv.write(Logic::from_bool(enb));
        wea_drv.write(Logic::from_bool(wea));
        addra_drv.write(Bits::from_usize(addra));
        addrb_drv.write(Bits::from_usize(addrb));
        dia_drv.write(Bits::from_u16(dia_val));

        exec.tick_clock(&mut clk);

        let exp = ref_step(&mut ref_mem, &mut ref_dob, ena, enb, wea, addra, addrb, dia_val);

        let ena_l      = Logic::from_bool(ena);
        let enb_l      = Logic::from_bool(enb);
        let wea_l      = Logic::from_bool(wea);
        let addra_bits = Bits::<8>::from_usize(addra);
        let addrb_bits = Bits::<8>::from_usize(addrb);
        let dia_bits   = Bits::<16>::from_u16(dia_val);
        let exp_bits   = Bits::<16>::from_u16(exp);
        let dob_read   = dob_obs.read();

        test.record_cycle(
            i,
            &[
                ("ena",   std::slice::from_ref(&ena_l)),
                ("enb",   std::slice::from_ref(&enb_l)),
                ("wea",   std::slice::from_ref(&wea_l)),
                ("addra", &addra_bits[..]),
                ("addrb", &addrb_bits[..]),
                ("dia",   &dia_bits[..]),
            ],
            &[("dob", &dob_read[..])],
        );
        expected_cycles.push(make_cycle(
            i,
            &[
                ("ena",   std::slice::from_ref(&ena_l)),
                ("enb",   std::slice::from_ref(&enb_l)),
                ("wea",   std::slice::from_ref(&wea_l)),
                ("addra", &addra_bits[..]),
                ("addrb", &addrb_bits[..]),
                ("dia",   &dia_bits[..]),
            ],
            &[("dob", &exp_bits[..])],
        ));
    }

    let expected = SimulationTrace::from_cycles(expected_cycles);
    test.finish_with_expected(&expected).assert_passed();
}
