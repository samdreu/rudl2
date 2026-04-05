use copper_core::{Bits, Logic};
use copper_sim::{HardwareTest, SimulationTrace, make_cycle};
use copper_macros::hardware;
use std::fs;
use std::path::Path;

fn bits_to_logic_vec<const N: usize>(bits: &Bits<N>) -> Vec<Logic> {
    bits.as_array().to_vec()
}

#[hardware]
fn mux_4to1(a: Bits<4>, b: Bits<4>, c: Bits<4>, d: Bits<4>, sel: Bits<2>) -> Bits<4> {
    match sel.as_u128() {
        0 => a,
        1 => b,
        2 => c,
        3 => d,
        _ => Bits::from_u128(0),
    }
}

fn main() {
    let a = Bits::<4>::from_u128(0b0001);
    let b = Bits::<4>::from_u128(0b0010);
    let c = Bits::<4>::from_u128(0b0100);
    let d = Bits::<4>::from_u128(0b1000);

    let selects = [
        Bits::<2>::from_u128(0),
        Bits::<2>::from_u128(1),
        Bits::<2>::from_u128(2),
        Bits::<2>::from_u128(3),
    ];

    println!("4-to-1 MUX test");
    println!("sel | output");
    println!("----+--------");

    // Generate Verilog from Rust source
    let verilog = copper_codegen::module_verilog!(mux_4to1);
    let verilog_path = "verilog/generated-verilog/mux_4to1.v";
    if let Some(parent) = Path::new(verilog_path).parent() {
        fs::create_dir_all(parent).expect("failed to create Verilog output directory");
    }
    fs::write(verilog_path, &verilog).expect("failed to write Verilog");
    println!("=== Generated Verilog ===\n{}", verilog);

    let mut test = HardwareTest::new("mux_4to1")
        .with_verilog(verilog_path)
        .with_waveform("waveforms/mux_4to1.vcd");

    let a_logic = bits_to_logic_vec(&a);
    let b_logic = bits_to_logic_vec(&b);
    let c_logic = bits_to_logic_vec(&c);
    let d_logic = bits_to_logic_vec(&d);

    let mut expected_cycles = Vec::new();

    for (cycle, sel) in selects.iter().enumerate() {
        let out = mux_4to1(a.clone(), b.clone(), c.clone(), d.clone(), sel.clone());
        println!(" {}  |  {:04b}", sel.as_u128(), out.as_u128());

        let sel_logic = bits_to_logic_vec(sel);
        let out_logic = bits_to_logic_vec(&out);

        test.record_cycle(
            cycle,
            &[
                ("a",   &a_logic),
                ("b",   &b_logic),
                ("c",   &c_logic),
                ("d",   &d_logic),
                ("sel", &sel_logic),
            ],
            &[("out", &out_logic)],
        );

        // Build expected trace from the correct values
        expected_cycles.push(make_cycle(
            cycle,
            &[
                ("a",   &a_logic),
                ("b",   &b_logic),
                ("c",   &c_logic),
                ("d",   &d_logic),
                ("sel", &sel_logic),
            ],
            &[("out", &out_logic)],
        ));
    }

    let expected = SimulationTrace::from_cycles(expected_cycles);
    test.finish_with_expected(&expected).assert_passed();
}
