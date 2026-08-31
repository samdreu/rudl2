//! **m3 — the memory family's traces, derived from the cycle-dataflow model and
//! pinned against BOTH implementations** (`design_docs/DERIVATION_TABLE.md` §5).
//!
//! The derivation table's memory rows were verdict **unchanged (predict)**: a
//! staging is a *closing* commit (its address/data/enable operands sample at the
//! staging edge, like a register's next-state), the result capture `q = data()`
//! is a *trailing* commit at the producing edge, and the port write publishes the
//! committed `q`. The corpus sweep only ever asserts sim ≡ SV — two
//! implementations of one source. This file asserts something stronger for two
//! representatives: **both equal the trace derived by hand from the model**, so
//! the "unchanged" verdict rests on the denotation, not on mutual agreement.
//!
//! Each test's expected trace is derived in its comment before it is asserted.
//! If either assertion fails, the model's memory-window clauses (table §8 item 4)
//! are wrong — fix the derivation, not the pin.

mod common;
use common::{verilator_available, verilator_command};

// The R+W representative, with a same-edge collision to pin ReadFirst. Included
// at top level (like tests/rv32i_integration.rs includes its CPU): the example
// brings its own imports and `MainClk`, which the ROM fixture below shares. Its
// `fn main` is `#[cfg(not(test))]`, so it compiles out here.
include!("../examples/memory/dual_port_ram.rs");

// The single-tick ROM shape (§5.4's widening #2 wrongly flagged it; the audit
// classifies it close+reg+mem+out[fwd]). Single source of truth: the fixture.
include!("fixtures/preloaded_rom_dut.rs");

/// Verilate `sv` as `top` and run a **per-cycle stimulus**: for each cycle, set
/// the named ports, then drive-then-clock (`clk=0; eval; clk=1; eval;`) and probe
/// — the same convention the simulator harness below uses, and the one every
/// equivalence test in this repo uses.
fn run_sv_stimulus(
    sv: &str,
    top: &str,
    probe: &str,
    cycles: &[&[(&str, u64)]],
) -> Vec<u64> {
    let work = std::env::temp_dir().join(format!("copper_m3_{top}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let sv_path = work.join(format!("{top}.sv"));
    std::fs::write(&sv_path, sv).unwrap();

    let mut tb = format!(
        "#include \"V{top}.h\"\n#include \"verilated.h\"\n#include <iostream>\n\
         int main(int c, char** v) {{ Verilated::commandArgs(c, v);\n\
         V{top}* t = new V{top}(); t->clk = 0; t->eval();\n"
    );
    for cycle in cycles {
        for (port, val) in *cycle {
            tb.push_str(&format!("t->{port} = {val}ULL;\n"));
        }
        tb.push_str(&format!(
            "t->clk=0; t->eval(); t->clk=1; t->eval(); \
             std::cout << (unsigned long long)t->{probe} << std::endl;\n"
        ));
    }
    tb.push_str("return 0; }\n");
    let tb_path = work.join("tb.cpp");
    std::fs::write(&tb_path, tb).unwrap();

    let out = verilator_command()
        .current_dir(&work)
        .args([
            "--cc", "--exe", "--build", "--top-module", top,
            "-Wno-DECLFILENAME", "-Wno-WIDTHEXPAND", "-CFLAGS", "-std=c++14",
        ])
        .arg(std::fs::canonicalize(&sv_path).unwrap())
        .arg(&tb_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "verilator build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let run = std::process::Command::new(work.join(format!("obj_dir/V{top}"))).output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&work);
    stdout.lines().filter_map(|l| l.trim().parse().ok()).collect()
}

fn transpile(src: &str, module: Option<&str>) -> String {
    copper_codegen::transpile_source(src, module, &copper_codegen::EmitConfig::default())
        .expect("transpile")
}

/// # Derivation (rom_from_fn: `rom[i] = 3i + 7`, READ_LAT = 1)
///
/// Pre-tick region: `rom.read(addr.read())` — the address feeds a staging, so
/// the region is closing-anchored and `addr` samples at the staging edge: the
/// read captured at edge N uses `addr_N`, the value driven before edge N.
/// Trailing region (executes at the opening of the cycle edge N opens):
/// `q = data()` commits `rom[addr_N]` at edge N; `data.write(q)` publishes the
/// committed value at the observation instant.
///
/// **Denotation: obs N = rom[addr_N] = 3·addr_N + 7.** No warm-up cycle: the
/// first staged read is captured at edge 1 and observed at obs 1.
#[test]
fn m3_rom_from_fn_matches_the_derived_denotation() {
    const N: usize = 13;
    let addr_seq: Vec<u64> = (1..=N as u64).map(|k| (5 * k + 3) % 16).collect();
    let derived: Vec<u64> = addr_seq.iter().map(|a| 3 * a + 7).collect();

    // Simulator, drive-then-clock.
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (addr_drv, addr_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (data_out, data_obs) = wire::<Bits<16>, MainClk>(Bits::zero());
    let dh = data_out.dirty_handle();
    let reads = vec![addr_in.wire_id()];
    exec.spawn_wired(rom_from_fn(clk.clone(), addr_in, data_out), vec![dh], reads);
    let sim: Vec<u64> = addr_seq
        .iter()
        .map(|&a| {
            addr_drv.write(Bits::from_usize(a as usize));
            exec.tick_clock(&mut clk);
            data_obs.read().as_u128() as u64
        })
        .collect();
    assert_eq!(
        sim, derived,
        "the SIMULATOR disagrees with the model's derived trace for rom_from_fn"
    );

    if !verilator_available() {
        return;
    }
    let sv = transpile(include_str!("fixtures/preloaded_rom_dut.rs"), Some("rom_from_fn"));
    let cycles: Vec<Vec<(&str, u64)>> =
        addr_seq.iter().map(|&a| vec![("addr", a)]).collect();
    let cycle_refs: Vec<&[(&str, u64)]> = cycles.iter().map(|c| c.as_slice()).collect();
    let hw = run_sv_stimulus(&sv, "rom_from_fn", "data", &cycle_refs);
    assert_eq!(
        hw, derived,
        "the TRANSPILED SV disagrees with the model's derived trace for rom_from_fn"
    );
}

/// # Derivation (dual_port_ram: 1R + 1W, READ_LAT = 1, ReadFirst)
///
/// Both stagings are closing commits at edge N with operands sampled there; the
/// capture `data = data()` is a trailing commit at edge N; the write publishes
/// committed `data`. ReadFirst: a read and a write to the same address at one
/// edge capture the OLD word. An unstaged cycle leaves `data` holding.
///
/// | cycle | drive (before the edge)                        | derived obs |
/// |-------|------------------------------------------------|-------------|
/// | 1     | write [5]=0xAAAA **and** read [5]              | 0x0000 (ReadFirst: old word) |
/// | 2     | read [5]                                       | 0xAAAA (edge-1 write visible) |
/// | 3     | no read staged                                 | 0xAAAA (hold) |
/// | 4     | write [7]=0x1234 **and** read [7]              | 0x0000 (ReadFirst again) |
/// | 5     | read [7]                                       | 0x1234 |
/// | 6     | read [5]                                       | 0xAAAA |
#[test]
fn m3_dual_port_ram_matches_the_derived_denotation() {
    let stim: Vec<Vec<(&str, u64)>> = vec![
        vec![("enable_a", 1), ("write_a", 1), ("addr_a", 5), ("data_in_a", 0xAAAA),
             ("enable_b", 1), ("addr_b", 5)],
        vec![("enable_a", 0), ("write_a", 0), ("addr_a", 0), ("data_in_a", 0),
             ("enable_b", 1), ("addr_b", 5)],
        vec![("enable_a", 0), ("write_a", 0), ("addr_a", 0), ("data_in_a", 0),
             ("enable_b", 0), ("addr_b", 0)],
        vec![("enable_a", 1), ("write_a", 1), ("addr_a", 7), ("data_in_a", 0x1234),
             ("enable_b", 1), ("addr_b", 7)],
        vec![("enable_a", 0), ("write_a", 0), ("addr_a", 0), ("data_in_a", 0),
             ("enable_b", 1), ("addr_b", 7)],
        vec![("enable_a", 0), ("write_a", 0), ("addr_a", 0), ("data_in_a", 0),
             ("enable_b", 1), ("addr_b", 5)],
    ];
    let derived: Vec<u64> = vec![0x0000, 0xAAAA, 0xAAAA, 0x0000, 0x1234, 0xAAAA];

    // Simulator, drive-then-clock — the example's own wiring.
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (ena_drv, ena_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (enb_drv, enb_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (wea_drv, wea_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (addra_drv, addra_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (addrb_drv, addrb_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (dia_drv, dia_in) = wire::<Bits<16>, MainClk>(Bits::zero());
    let (dob_out, dob_obs) = wire::<Bits<16>, MainClk>(Bits::zero());
    let dh = dob_out.dirty_handle();
    let reads = vec![
        ena_in.wire_id(), enb_in.wire_id(), wea_in.wire_id(),
        addra_in.wire_id(), addrb_in.wire_id(), dia_in.wire_id(),
    ];
    exec.spawn_wired(
        dual_port_ram(clk.clone(), ena_in, enb_in, wea_in, addra_in, addrb_in, dia_in, dob_out),
        vec![dh],
        reads,
    );
    let sim: Vec<u64> = stim
        .iter()
        .map(|cycle| {
            for (port, val) in cycle {
                match *port {
                    "enable_a" => ena_drv.write(if *val == 1 { Logic::One } else { Logic::Zero }),
                    "enable_b" => enb_drv.write(if *val == 1 { Logic::One } else { Logic::Zero }),
                    "write_a" => wea_drv.write(if *val == 1 { Logic::One } else { Logic::Zero }),
                    "addr_a" => addra_drv.write(Bits::from_usize(*val as usize)),
                    "addr_b" => addrb_drv.write(Bits::from_usize(*val as usize)),
                    "data_in_a" => dia_drv.write(Bits::from_usize(*val as usize)),
                    _ => unreachable!(),
                }
            }
            exec.tick_clock(&mut clk);
            dob_obs.read().as_u128() as u64
        })
        .collect();
    assert_eq!(
        sim, derived,
        "the SIMULATOR disagrees with the model's derived trace for dual_port_ram"
    );

    if !verilator_available() {
        return;
    }
    let sv = transpile(include_str!("../examples/memory/dual_port_ram.rs"), None);
    let cycle_refs: Vec<&[(&str, u64)]> = stim.iter().map(|c| c.as_slice()).collect();
    let hw = run_sv_stimulus(&sv, "dual_port_ram", "data_out_b", &cycle_refs);
    assert_eq!(
        hw, derived,
        "the TRANSPILED SV disagrees with the model's derived trace for dual_port_ram"
    );
}
