use std::cell::UnsafeCell;
use std::ops::{Index, IndexMut};
use crate::types::{Clock, ClockDomain};

/// Controls read-during-write behavior when the same address is read and written in the same cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Output register captures old data before the write commits.
    /// Default — matches most FPGA block RAMs (Xilinx RAMB, Intel M20K).
    ReadFirst,
    /// Writes commit before output capture; same-address read sees the new value.
    WriteFirst,
}

/// Controls whether the read output is registered (synchronous) or combinational (asynchronous).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// `rp.output()` returns the registered value captured at the last clock edge.
    /// `rp[addr]` drives the read address and returns current committed data.
    /// Default — matches FPGA block RAM read port behavior.
    Sync,
    /// `rp[addr]` returns data immediately (combinational). `rp.output()` always returns `None`.
    Async,
}

struct MemoryInner<T, const R: usize, const W: usize> {
    data: Vec<T>, // Actual storage for the data.
    /// W staged writes, one per write port. Committed atomically on the next clock edge.
    /// Like a reservation station for each write port's pending update. Only the last staged write
    /// to a given address will survive, matching real hardware behavior.
    staging: [Option<(usize, T)>; W],
    /// R registered read addresses — updated by `rp[addr]`, captured at each clock edge.
    read_addr: [usize; R],
    /// R registered read outputs — `None` until the first clock edge fires (hardware X state).
    output_reg: [Option<T>; R],
    /// Tracks whether each write port has been used this cycle. Reset each clock edge.
    /// Used by `debug_assert!` to catch designs that drive the same write port twice in a cycle,
    /// which is impossible in real hardware (a physical write port has one data/address bus).
    write_used: [bool; W],
    last_cycle: u64,
}

/// A clocked, multi-port memory that models Verilog block RAM semantics.
///
/// `R` = number of independent read ports.
/// `W` = number of independent write ports.
/// `D` = clock domain marker.
///
/// # Port access
/// - `mem.read_port::<I>()` — returns read port I (for I in 0..R)
/// - `mem.write_port::<I>()` — returns write port I (for I in 0..W)
///
/// # Array indexing
/// - `rp[addr]` — drives the read address; returns current committed data
///   (or staged value in write-first mode if a write to `addr` is pending)
/// - `rp.output()` — returns the registered output from the last clock edge (`None` before
///   the first edge — hardware X state)
/// - `wp[addr] = val` — queues a write; committed at the next clock edge
/// - `wp[addr] += n` — read-modify-write: reads committed value, stages the modified result
///
/// # Simple dual-port example
/// ```rust,ignore
/// let mut mem = Memory::<u8, 1, 1, MainClk>::new(clk.clone(), 256);
/// loop {
///     let _ = mem.read_port::<0>()[rd_addr];     // drive read address
///     emit!(*mem.read_port::<0>().output().unwrap_or(&0)); // registered output
///     clk.tick().await;                           // edge: write commits, output_reg captured
///     if we { mem.write_port::<0>()[wr_addr] = din; }
/// }
/// ```
///
/// # ROM initialization
/// Use `from_contents` or `from_fn` — do not use `IndexMut` for multi-entry init since each
/// write port holds a single staging slot (only the last staged value survives).
pub struct Memory<T, const R: usize, const W: usize, D: ClockDomain> {
    inner: UnsafeCell<MemoryInner<T, R, W>>,
    clock: Clock<D>,
    write_mode: WriteMode,
    read_mode: ReadMode,
}

// SAFETY: Memory is only created and accessed from the single-threaded simulation executor.
// The UnsafeCell never crosses a thread boundary.
unsafe impl<T: Send, const R: usize, const W: usize, D: ClockDomain> Send
    for Memory<T, R, W, D>
{
}

impl<T: Clone, const R: usize, const W: usize, D: ClockDomain> Memory<T, R, W, D> {
    /// If the clock has advanced, commit staged writes and update output registers.
    /// This is the Rust equivalent of `always @(posedge clk)`.
    fn sync_if_advanced(&self) {
        let cycle = self.clock.cycle();
        // SAFETY: single-threaded simulation — no concurrent access.
        let inner = unsafe { &mut *self.inner.get() };
        if cycle <= inner.last_cycle {
            return;
        }

        match self.write_mode {
            WriteMode::ReadFirst => {
                // Capture output registers BEFORE committing writes.
                if self.read_mode == ReadMode::Sync {
                    for i in 0..R {
                        inner.output_reg[i] = Some(inner.data[inner.read_addr[i]].clone());
                    }
                }
                for staged in inner.staging.iter_mut() {
                    if let Some((addr, val)) = staged.take() {
                        inner.data[addr] = val;
                    }
                }
            }
            WriteMode::WriteFirst => {
                // Commit writes first, THEN capture output registers.
                for staged in inner.staging.iter_mut() {
                    if let Some((addr, val)) = staged.take() {
                        inner.data[addr] = val;
                    }
                }
                if self.read_mode == ReadMode::Sync {
                    for i in 0..R {
                        inner.output_reg[i] = Some(inner.data[inner.read_addr[i]].clone());
                    }
                }
            }
        }
        inner.write_used = [false; W];
        inner.last_cycle = cycle;
    }

    /// Get read port I. Multiple read ports may be used independently in the same cycle.
    pub fn read_port<const I: usize>(&self) -> ReadPort<'_, T, R, W, D, I> {
        ReadPort { mem: self }
    }

    /// Get write port I.
    pub fn write_port<const I: usize>(&self) -> WritePort<'_, T, R, W, D, I> {
        WritePort { mem: self }
    }

    /// Switch to write-first mode: same-address reads see the pending write value.
    pub fn write_first(mut self) -> Self {
        self.write_mode = WriteMode::WriteFirst;
        self
    }

    /// Switch to asynchronous (combinational) read. `output()` always returns `None`.
    pub fn async_read(mut self) -> Self {
        self.read_mode = ReadMode::Async;
        self
    }

    pub fn size(&self) -> usize {
        unsafe { (*self.inner.get()).data.len() }
    }

    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    pub fn read_mode(&self) -> ReadMode {
        self.read_mode
    }
}

impl<T: Clone + Default, const R: usize, const W: usize, D: ClockDomain> Memory<T, R, W, D> {
    fn make_inner(data: Vec<T>, cycle: u64) -> MemoryInner<T, R, W> {
        MemoryInner {
            data,
            staging: std::array::from_fn(|_| None),
            read_addr: [0; R],
            output_reg: std::array::from_fn(|_| None), // X — undefined until first edge
            write_used: [false; W],
            last_cycle: cycle,
        }
    }

    /// Create a zeroed memory with `size` elements.
    /// Defaults: sync read, read-first write mode.
    pub fn new(clock: Clock<D>, size: usize) -> Self {
        let cycle = clock.cycle();
        Memory {
            inner: UnsafeCell::new(Self::make_inner(vec![T::default(); size], cycle)),
            clock,
            write_mode: WriteMode::ReadFirst,
            read_mode: ReadMode::Sync,
        }
    }

    /// Create a memory pre-loaded with `data`.
    /// All entries are written directly to storage — no staging, no clock edge required.
    /// Use for ROM initialization and lookup tables.
    pub fn from_contents(clock: Clock<D>, data: Vec<T>) -> Self {
        let cycle = clock.cycle();
        Memory {
            inner: UnsafeCell::new(Self::make_inner(data, cycle)),
            clock,
            write_mode: WriteMode::ReadFirst,
            read_mode: ReadMode::Sync,
        }
    }

    /// Create a memory of `size` elements where `f(address)` computes each initial value.
    pub fn from_fn(clock: Clock<D>, size: usize, mut f: impl FnMut(usize) -> T) -> Self {
        let data = (0..size).map(|i| f(i)).collect();
        Self::from_contents(clock, data)
    }

}

// ── Read port ─────────────────────────────────────────────────────────────────

/// A handle to one of the R read ports. Lifetime tied to the `Memory` it came from.
/// `I` identifies which port (0..R).
pub struct ReadPort<'a, T, const R: usize, const W: usize, D: ClockDomain, const I: usize> {
    mem: &'a Memory<T, R, W, D>,
}

impl<'a, T: Clone + Default, const R: usize, const W: usize, D: ClockDomain, const I: usize>
    ReadPort<'a, T, R, W, D, I>
{
    /// Clone a value out of the memory at `addr`.
    /// Also records `addr` for output-register capture at the next clock edge (sync mode).
    pub fn read(&self, addr: usize) -> T {
        self[addr].clone()
    }

    /// The registered output captured at the last clock edge.
    ///
    /// Returns `None` before the first clock edge — the output register is undefined (X)
    /// until the first posedge, matching real block RAM behavior.
    ///
    /// Always returns `None` in async read mode (no output register).
    pub fn output(&self) -> Option<&T> {
        self.mem.sync_if_advanced();
        // SAFETY: single-threaded — no mutable alias while this shared ref lives.
        unsafe { (*self.mem.inner.get()).output_reg[I].as_ref() }
    }
}

/// `rp[addr]` — drive the read address and return current committed data.
///
/// - Records `addr` so it is captured into the output register at the next clock edge (sync mode).
/// - In write-first mode: if any write port has `addr` staged, returns that pending value instead
///   of the committed data (same-cycle read-after-write visibility).
/// - Calls `sync_if_advanced` so state is always up-to-date.
impl<'a, T: Clone + Default, const R: usize, const W: usize, D: ClockDomain, const I: usize>
    Index<usize> for ReadPort<'a, T, R, W, D, I>
{
    type Output = T;

    fn index(&self, addr: usize) -> &T {
        self.mem.sync_if_advanced();
        // SAFETY: single-threaded — no mutable alias while this shared ref lives.
        let inner = unsafe { &mut *self.mem.inner.get() };
        inner.read_addr[I] = addr;

        // Write-first: return the staged value from any write port if address matches.
        if self.mem.write_mode == WriteMode::WriteFirst {
            for staged in inner.staging.iter() {
                if let Some((wr_addr, val)) = staged {
                    if *wr_addr == addr {
                        return val;
                    }
                }
            }
        }

        &inner.data[addr]
    }
}

// ── Write port ────────────────────────────────────────────────────────────────

/// A handle to one of the W write ports. Lifetime tied to the `Memory` it came from.
/// `I` identifies which port (0..W).
pub struct WritePort<'a, T, const R: usize, const W: usize, D: ClockDomain, const I: usize> {
    mem: &'a Memory<T, R, W, D>,
}

impl<'a, T: Clone, const R: usize, const W: usize, D: ClockDomain, const I: usize>
    WritePort<'a, T, R, W, D, I>
{
    /// Stage a write to `addr`. Committed at the next clock edge.
    /// Panics in debug builds if this port is driven more than once in the same cycle —
    /// a physical write port has one address/data bus; driving it twice is a hardware error.
    pub fn write(&self, addr: usize, value: T) {
        self.mem.sync_if_advanced();
        // SAFETY: single-threaded.
        let inner = unsafe { &mut *self.mem.inner.get() };
        debug_assert!(
            !inner.write_used[I],
            "write port {} driven more than once in the same cycle (hardware violation)",
            I
        );
        inner.write_used[I] = true;
        inner.staging[I] = Some((addr, value));
    }
}

/// `wp[addr]` — return the current view from this write port's perspective.
///
/// In write-first mode: if this port has `addr` staged, returns the staged (pending) value.
/// Otherwise returns the committed data.
impl<'a, T: Clone + Default, const R: usize, const W: usize, D: ClockDomain, const I: usize>
    Index<usize> for WritePort<'a, T, R, W, D, I>
{
    type Output = T;

    fn index(&self, addr: usize) -> &T {
        self.mem.sync_if_advanced();
        // SAFETY: single-threaded.
        let inner = unsafe { &*self.mem.inner.get() };
        if self.mem.write_mode == WriteMode::WriteFirst {
            if let Some((wr_addr, val)) = &inner.staging[I] {
                if *wr_addr == addr {
                    return val;
                }
            }
        }
        &inner.data[addr]
    }
}

/// `wp[addr] = val` — stage a write via assignment syntax.
///
/// Also supports read-modify-write: `wp[addr] += n` reads the committed value,
/// stages it in the slot, then adds `n` to the staged copy.
///
/// `wp[addr] = val` — stage a write via assignment syntax.
/// Panics in debug builds if this port is driven more than once in the same cycle.
impl<'a, T: Clone + Default, const R: usize, const W: usize, D: ClockDomain, const I: usize>
    IndexMut<usize> for WritePort<'a, T, R, W, D, I>
{
    fn index_mut(&mut self, addr: usize) -> &mut T {
        self.mem.sync_if_advanced();
        // SAFETY: single-threaded, no read alias active simultaneously.
        let inner = unsafe { &mut *self.mem.inner.get() };
        debug_assert!(
            !inner.write_used[I],
            "write port {} driven more than once in the same cycle (hardware violation)",
            I
        );
        inner.write_used[I] = true;
        // Seed staging from committed value so read-modify-write works correctly.
        inner.staging[I] = Some((addr, inner.data[addr].clone()));
        &mut inner.staging[I].as_mut().unwrap().1
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Clock, ClockDomain};

    struct TestClk;
    impl ClockDomain for TestClk {}

    fn clk() -> Clock<TestClk> {
        Clock::<TestClk>::new()
    }

    // ── Construction ─────────────────────────────────────────────────────────

    #[test]
    fn from_contents_preloads_data() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::from_contents(clock, vec![0x11, 0x22, 0x33]);
        let rp = memory.read_port::<0>();
        assert_eq!(rp.read(0), 0x11);
        assert_eq!(rp.read(1), 0x22);
        assert_eq!(rp.read(2), 0x33);
    }

    #[test]
    fn from_fn_computes_each_entry() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::from_fn(clock, 4, |i| (i * 10) as u8);
        let rp = memory.read_port::<0>();
        assert_eq!(rp.read(0), 0);
        assert_eq!(rp.read(1), 10);
        assert_eq!(rp.read(2), 20);
        assert_eq!(rp.read(3), 30);
    }

    #[test]
    fn new_zeroed() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock, 4);
        let rp = memory.read_port::<0>();
        for i in 0..4 {
            assert_eq!(rp.read(i), 0);
        }
    }

    // ── output_reg X semantics ────────────────────────────────────────────────

    #[test]
    fn output_is_none_before_first_clock_edge() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock, 4);
        assert_eq!(memory.read_port::<0>().output(), None);
    }

    #[test]
    fn output_some_after_first_clock_edge() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4);
        clock.advance();
        // read_addr defaults to 0, data[0] = 0
        assert_eq!(memory.read_port::<0>().output(), Some(&0u8));
    }

    // ── Sync write (staged commit) ────────────────────────────────────────────

    #[test]
    fn write_commits_on_next_clock_edge() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4);

        memory.write_port::<0>().write(2, 0xAB);
        assert_eq!(memory.read_port::<0>().read(2), 0); // not yet committed

        clock.advance();
        assert_eq!(memory.read_port::<0>().read(2), 0xAB);
    }

    #[test]
    #[should_panic(expected = "write port 0 driven more than once in the same cycle")]
    fn write_port_reuse_in_same_cycle_panics() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock, 4);

        memory.write_port::<0>().write(1, 0x10);
        memory.write_port::<0>().write(1, 0x20); // second drive on same port — hardware violation
    }

    #[test]
    fn write_port_index_syntax_stages_write() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4);

        {
            let mut wp = memory.write_port::<0>();
            wp[3] = 0x42;
        }
        assert_eq!(memory.read_port::<0>().read(3), 0); // not committed yet

        clock.advance();
        assert_eq!(memory.read_port::<0>().read(3), 0x42);
    }

    #[test]
    fn write_port_read_modify_write() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::from_contents(clock.clone(), vec![10, 20, 30]);

        {
            let mut wp = memory.write_port::<0>();
            wp[1] += 5; // staging initialized from 20, becomes 25
        }
        clock.advance();
        assert_eq!(memory.read_port::<0>().read(1), 25);
    }

    // ── Sync read-first (default) ─────────────────────────────────────────────

    #[test]
    fn sync_read_first_output_captures_old_value() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4);

        // Write and read same address in same cycle (read-first: output captures pre-write data).
        memory.write_port::<0>().write(0, 42);
        let _ = memory.read_port::<0>()[0]; // set read_addr = 0

        clock.advance();
        // output_reg should have 0 (captured before 42 was committed)
        assert_eq!(memory.read_port::<0>().output(), Some(&0u8));
        // committed data is now 42
        assert_eq!(memory.read_port::<0>().read(0), 42);
    }

    #[test]
    fn sync_read_first_output_updates_each_cycle() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4);

        memory.write_port::<0>().write(0, 10);
        let _ = memory.read_port::<0>()[0];
        clock.advance();
        assert_eq!(memory.read_port::<0>().output(), Some(&0u8)); // pre-write capture

        let _ = memory.read_port::<0>()[0];
        clock.advance();
        assert_eq!(memory.read_port::<0>().output(), Some(&10u8)); // now captures committed 10
    }

    // ── Sync write-first ──────────────────────────────────────────────────────

    #[test]
    fn write_first_read_port_sees_staged_value() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock, 4).write_first();

        memory.write_port::<0>().write(0, 42);
        // Same-cycle read via read port should return staged 42 (write-first)
        assert_eq!(memory.read_port::<0>().read(0), 42);
    }

    #[test]
    fn write_first_output_captures_new_value() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4).write_first();

        memory.write_port::<0>().write(0, 42);
        let _ = memory.read_port::<0>()[0]; // read_addr = 0
        clock.advance();
        // write committed first, then output captured — should be 42
        assert_eq!(memory.read_port::<0>().output(), Some(&42u8));
    }

    #[test]
    fn write_first_different_address_unaffected() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock, 4).write_first();

        memory.write_port::<0>().write(1, 42);
        assert_eq!(memory.read_port::<0>().read(0), 0); // different addr, write invisible
    }

    // ── Async read ────────────────────────────────────────────────────────────

    #[test]
    fn async_read_returns_immediate_data() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4).async_read();

        memory.write_port::<0>().write(0, 42);
        assert_eq!(memory.read_port::<0>().read(0), 0); // read-first: write not yet visible
        clock.advance();
        assert_eq!(memory.read_port::<0>().read(0), 42);
    }

    #[test]
    fn async_output_always_none() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4).async_read();
        clock.advance();
        assert_eq!(memory.read_port::<0>().output(), None);
    }

    // ── Multiple independent ports ────────────────────────────────────────────

    #[test]
    fn two_read_ports_independent() {
        let clock = clk();
        let memory =
            Memory::<u8, 2, 1, TestClk>::from_contents(clock, vec![1, 2, 3, 4]);

        assert_eq!(memory.read_port::<0>().read(0), 1);
        assert_eq!(memory.read_port::<1>().read(3), 4);
    }

    #[test]
    fn two_write_ports_both_commit() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 2, TestClk>::new(clock.clone(), 4);

        memory.write_port::<0>().write(0, 0xAA);
        memory.write_port::<1>().write(1, 0xBB);
        clock.advance();

        assert_eq!(memory.read_port::<0>().read(0), 0xAA);
        assert_eq!(memory.read_port::<0>().read(1), 0xBB);
    }

    #[test]
    fn two_write_ports_conflicting_addr_both_commit_independently() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 2, TestClk>::new(clock.clone(), 4);

        // Both write to addr 0 in same cycle — both stage into separate slots.
        // Both commit (last slot in iteration order wins in data[]).
        memory.write_port::<0>().write(0, 0x11);
        memory.write_port::<1>().write(0, 0x22);
        clock.advance();

        // Port 0 and port 1 both staged to addr 0; port 1 (index 1) commits last.
        let val = memory.read_port::<0>().read(0);
        assert!(val == 0x11 || val == 0x22, "one of the two ports committed");
    }

    // ── Registered read per port ──────────────────────────────────────────────

    #[test]
    fn each_read_port_has_independent_output_register() {
        let mut clock = clk();
        let memory =
            Memory::<u8, 2, 1, TestClk>::from_contents(clock.clone(), vec![0xAA, 0xBB, 0xCC, 0xDD]);

        let _ = memory.read_port::<0>()[0]; // port 0 reads addr 0
        let _ = memory.read_port::<1>()[3]; // port 1 reads addr 3
        clock.advance();

        assert_eq!(memory.read_port::<0>().output(), Some(&0xAAu8));
        assert_eq!(memory.read_port::<1>().output(), Some(&0xDDu8));
    }
}
