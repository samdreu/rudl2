use copper_core::{Clock, ClockDomain, Logic, Memory};
use copper_sim::{SimulationTrace, verify_with_verilator};
use std::process::Command;

struct FifoClk;
impl ClockDomain for FifoClk {}

#[derive(Debug, Clone, Copy)]
struct FifoStep {
    wr_en: bool,
    rd_en: bool,
    din: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FifoOutputs {
    dout: u8,
    empty: bool,
    full: bool,
    valid: bool,
    count: u8,
}

struct MemoryBackedFifo {
    clock: Clock<FifoClk>,
    mem: Memory<u8, 1, 1, FifoClk>,
    read_ptr: usize,
    write_ptr: usize,
    count: usize,
}

impl MemoryBackedFifo {
    const DEPTH: usize = 4;

    fn new() -> Self {
        let clock = Clock::<FifoClk>::new();
        let mem = Memory::<u8, 1, 1, FifoClk>::new(clock.clone(), Self::DEPTH);
        Self {
            clock,
            mem,
            read_ptr: 0,
            write_ptr: 0,
            count: 0,
        }
    }

    fn cycle(&mut self, wr_en: bool, rd_en: bool, din: u8) -> FifoOutputs {
        let can_write = wr_en && self.count < Self::DEPTH;
        let can_read = rd_en && self.count > 0;

        if can_write {
            self.mem.write_port::<0>().write(self.write_ptr, din);
            self.write_ptr = (self.write_ptr + 1) % Self::DEPTH;
        }

        if can_read {
            self.read_ptr = (self.read_ptr + 1) % Self::DEPTH;
        }

        self.count += can_write as usize;
        self.count -= can_read as usize;

        self.clock.advance();

        let empty = self.count == 0;
        let full = self.count == Self::DEPTH;
        let dout = if empty {
            0
        } else {
            self.mem.read_port::<0>().read(self.read_ptr)
        };

        FifoOutputs {
            dout,
            empty,
            full,
            valid: !empty,
            count: self.count as u8,
        }
    }
}

fn has_verilator() -> bool {
    Command::new("verilator")
        .arg("--version")
        .env_remove("VERILATOR_ROOT")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn bit(v: bool) -> Vec<Logic> {
    vec![if v { Logic::One } else { Logic::Zero }]
}

fn u8_to_logic_vec(v: u8) -> Vec<Logic> {
    (0..8)
        .map(|i| if (v >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

fn u3_to_logic_vec(v: u8) -> Vec<Logic> {
    (0..3)
        .map(|i| if (v >> i) & 1 == 1 { Logic::One } else { Logic::Zero })
        .collect()
}

#[test]
fn fifo_memory_new_matches_verilog_cycle_by_cycle() {
    if !has_verilator() {
        eprintln!("Skipping Verilator test: 'verilator' not found in PATH");
        return;
    }

    let sequence = [
        FifoStep { wr_en: false, rd_en: false, din: 0x00 },
        FifoStep { wr_en: true, rd_en: false, din: 0x11 },
        FifoStep { wr_en: true, rd_en: false, din: 0x22 },
        FifoStep { wr_en: true, rd_en: false, din: 0x33 },
        FifoStep { wr_en: true, rd_en: false, din: 0x44 },
        FifoStep { wr_en: true, rd_en: false, din: 0x55 },
        FifoStep { wr_en: false, rd_en: true, din: 0x00 },
        FifoStep { wr_en: true, rd_en: true, din: 0x66 },
        FifoStep { wr_en: true, rd_en: true, din: 0x77 },
        FifoStep { wr_en: false, rd_en: true, din: 0x00 },
        FifoStep { wr_en: false, rd_en: true, din: 0x00 },
        FifoStep { wr_en: false, rd_en: true, din: 0x00 },
        FifoStep { wr_en: false, rd_en: true, din: 0x00 },
        FifoStep { wr_en: true, rd_en: true, din: 0x88 },
    ];

    let mut model = MemoryBackedFifo::new();
    let mut trace = SimulationTrace::new();

    for (cycle, step) in sequence.iter().enumerate() {
        let out = model.cycle(step.wr_en, step.rd_en, step.din);
        trace.add_cycle(
            cycle + 1,
            vec![
                ("wr_en".to_string(), bit(step.wr_en)),
                ("rd_en".to_string(), bit(step.rd_en)),
                ("din".to_string(), u8_to_logic_vec(step.din)),
            ],
            vec![
                ("dout".to_string(), u8_to_logic_vec(out.dout)),
                ("empty".to_string(), bit(out.empty)),
                ("full".to_string(), bit(out.full)),
                ("valid".to_string(), bit(out.valid)),
                ("count".to_string(), u3_to_logic_vec(out.count)),
            ],
        );
    }

    let verified = verify_with_verilator("verilog/fifo_mem_new.v", "fifo_mem_new", &trace)
        .expect("Verilator verification should run successfully");
    assert!(verified, "Expected Verilator comparison to pass");
}
