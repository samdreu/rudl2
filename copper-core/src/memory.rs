use std::cell::UnsafeCell;
use std::ops::{Index, IndexMut};
use crate::types::{Clock, ClockDomain};

/// Controls what data is returned when reading and writing the same address in the same cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Read returns old (pre-write) data. Default — matches most block RAMs.
    ReadFirst,
    /// Read returns new (post-write) data when read/write addresses match in the same cycle.
    WriteFirst,
}

/// Controls whether reads are registered (one-cycle latency) or combinational (immediate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// `output()` returns a registered value captured at the last clock edge.
    /// The address used in the most recent `mem[addr]` before that edge is what gets captured.
    /// Default — matches block RAM read port behavior.
    Sync,
    /// `mem[addr]` returns data immediately with no output register. `output()` always
    /// returns `None` in this mode.
    Async,
}

struct MemoryInner<T> {
    data: Vec<T>,
    /// Pending write queued by `IndexMut`. Committed automatically at the next clock edge.
    staging: Option<(usize, T)>,
    /// Registered read output for `Sync` mode.
    /// `None` = X/uninitialized (before the first clock edge fires after construction).
    output_reg: Option<T>,
    /// Read address recorded by the last `Index` call. Captured into `output_reg` at the edge.
    read_addr: usize,
    /// Clock cycle at which state was last committed.
    last_seen_cycle: u64,
}

/// A clocked memory that models Verilog block RAM semantics.
///
/// # Port model — Simple Dual-Port block RAM
///
/// This memory models a **simple dual-port block RAM**: an independent read port and
/// write port, each with its own address. This matches the most common FPGA block RAM
/// primitive (e.g., Xilinx RAMB36/RAMB18 in SDP mode, Intel M20K in Simple Dual Port mode).
///
/// | Port  | Address        | Data direction | Latency       |
/// |-------|----------------|----------------|---------------|
/// | Read  | `mem[rd_addr]` | out → `output()` | 1 cycle (Sync) or 0 (Async) |
/// | Write | `mem[wr_addr] = val` | in | committed at next clock edge |
///
/// Read and write addresses are independent — they may differ in the same cycle.
/// Same-address behavior (read-during-write) is governed by `WriteMode`.
///
/// # Usage inside a `#[hardware]` module
/// ```rust,ignore
/// let mut mem = Memory::<u8, MainClk>::new(clk.clone(), 256);
/// loop {
///     let _ = mem[rd_addr];                   // drive read address, records it for sync capture
///     emit!(*mem.output().unwrap());          // output_reg from previous clock edge (None before first edge)
///     clk.tick().await;                       // clock edge: pending write committed, output_reg captured
///     if we { mem[wr_addr] = din; }           // queue write for next edge
/// }
/// ```
///
/// # ROM initialization
/// Use `from_contents` or `from_fn` to preload data at construction time.
/// **Do not** use `mem[i] = v` for initialization — `IndexMut` queues a single staged
/// write, so only the last assignment per construction survives to the first clock edge.
///
/// # Builder
/// ```rust,ignore
/// Memory::<u8, MainClk>::new(clk.clone(), 256)
///     .write_first()     // same-address read sees pending write value in same cycle
///     .async_read()      // mem[addr] is combinational; output() always returns None
///     .with_reset(0xFF)  // mem.reset() drives output register to 0xFF and cancels pending write
/// ```
pub struct Memory<T, Domain: ClockDomain> {
    size: usize,
    // SAFETY invariant: only ever accessed from the single simulation thread.
    inner: UnsafeCell<MemoryInner<T>>,
    write_mode: WriteMode,
    read_mode: ReadMode,
    reset_value: Option<T>,
    clock: Clock<Domain>,
}

// SAFETY: Memory is only created and accessed from the single-threaded simulation executor.
// The UnsafeCell never crosses a thread boundary.
unsafe impl<T: Send, Domain: ClockDomain> Send for Memory<T, Domain> {}

impl<T: Clone + Default, Domain: ClockDomain> Memory<T, Domain> {

    fn make_inner(data: Vec<T>, cycle: u64) -> MemoryInner<T> {
        MemoryInner {
            data,
            staging: None,
            output_reg: None, // X — undefined until the first clock edge fires
            read_addr: 0,
            last_seen_cycle: cycle,
        }
    }

    /// Create a new zeroed memory tied to `clock` with `size` elements.
    /// Defaults: sync read, read-first write mode, no reset.
    pub fn new(clock: Clock<Domain>, size: usize) -> Self {
        let cycle = clock.cycle();
        Memory {
            size,
            inner: UnsafeCell::new(Self::make_inner(vec![T::default(); size], cycle)),
            write_mode: WriteMode::ReadFirst,
            read_mode: ReadMode::Sync,
            reset_value: None,
            clock,
        }
    }

    /// Create a memory pre-loaded with `data`. The length of `data` sets the size.
    /// All entries are written directly to storage — no staging, no clock edge required.
    /// Use this for ROM initialization and pre-loaded lookup tables.
    pub fn from_contents(clock: Clock<Domain>, data: Vec<T>) -> Self {
        let cycle = clock.cycle();
        let size = data.len();
        Memory {
            size,
            inner: UnsafeCell::new(Self::make_inner(data, cycle)),
            write_mode: WriteMode::ReadFirst,
            read_mode: ReadMode::Sync,
            reset_value: None,
            clock,
        }
    }

    /// Create a memory of `size` elements where each entry is initialized by `f(address)`.
    /// All entries are written directly to storage, same as `from_contents`.
    pub fn from_fn(clock: Clock<Domain>, size: usize, mut f: impl FnMut(usize) -> T) -> Self {
        let data = (0..size).map(|i| f(i)).collect();
        Self::from_contents(clock, data)
    }

    /// Switch to write-first mode: `mem[addr]` returns the pending write value when
    /// `addr` matches the queued write address in the same cycle.
    pub fn write_first(mut self) -> Self {
        self.write_mode = WriteMode::WriteFirst;
        self
    }

    /// Switch to asynchronous read: `mem[addr]` is purely combinational, no output register.
    /// `output()` always returns `None` in this mode.
    pub fn async_read(mut self) -> Self {
        self.read_mode = ReadMode::Async;
        self
    }

    /// Enable synchronous reset. `reset()` restores the output register to `value`
    /// and cancels any pending write.
    pub fn with_reset(mut self, value: T) -> Self {
        self.reset_value = Some(value);
        self
    }

    // ── Internal sync ─────────────────────────────────────────────────────────

    /// Check whether the clock has advanced since the last committed access. If it has,
    /// commit the pending write and (in Sync mode) capture the output register —
    /// exactly what a Verilog `always @(posedge clk)` block does.
    ///
    /// Called at the start of every `Index`, `IndexMut`, and `output()` access so that
    /// state is always up-to-date, even in write-only or output-only cycles.
    fn sync_if_advanced(&self) {
        let current = self.clock.cycle();
        // SAFETY: single-threaded simulation — no concurrent access.
        let inner = unsafe { &mut *self.inner.get() };
        if current <= inner.last_seen_cycle {
            return;
        }
        match (self.read_mode, self.write_mode) {
            (ReadMode::Sync, WriteMode::ReadFirst) => {
                // Capture output from pre-write data, then commit write.
                inner.output_reg = Some(inner.data[inner.read_addr].clone());
                if let Some((addr, val)) = inner.staging.take() {
                    inner.data[addr] = val;
                }
            }
            (ReadMode::Sync, WriteMode::WriteFirst) => {
                // Commit write first, then capture output from updated data.
                if let Some((addr, val)) = inner.staging.take() {
                    inner.data[addr] = val;
                }
                inner.output_reg = Some(inner.data[inner.read_addr].clone());
            }
            (ReadMode::Async, _) => {
                // No output register; just commit the write.
                if let Some((addr, val)) = inner.staging.take() {
                    inner.data[addr] = val;
                }
            }
        }
        inner.last_seen_cycle = current;
    }

    // ── Outputs ───────────────────────────────────────────────────────────────

    /// The registered read output (Sync mode).
    ///
    /// Returns `None` until the first clock edge fires after construction — matching real
    /// hardware where the output register is undefined (X) before the first posedge.
    /// After the first edge, returns `Some(&data)` reflecting the read address captured
    /// at that edge.
    ///
    /// Also triggers a sync, so calling this in a write-only cycle (no `mem[addr]` read)
    /// still correctly updates the output register for the latest clock edge.
    ///
    /// Always returns `None` in `Async` mode — use `mem[addr]` directly instead.
    pub fn output(&self) -> Option<&T> {
        self.sync_if_advanced();
        // SAFETY: single-threaded, no aliased mutable access.
        unsafe { (*self.inner.get()).output_reg.as_ref() }
    }

    /// Reset the output register to the configured reset value and cancel any pending write.
    /// No-op if `with_reset()` was not called.
    pub fn reset(&mut self) {
        if let Some(ref rv) = self.reset_value.clone() {
            // SAFETY: we have &mut self.
            let inner = unsafe { &mut *self.inner.get() };
            inner.output_reg = Some(rv.clone());
            inner.staging = None;
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    pub fn read_mode(&self) -> ReadMode {
        self.read_mode
    }
}

/// Read from the memory. Automatically commits any pending write if the clock has
/// advanced since the last access.
///
/// Also records `index` as the read address so Sync mode can capture it into the
/// output register at the next clock edge.
///
/// In `WriteFirst` mode: if `index` matches the address of the pending write, returns
/// the pending (not-yet-committed) write value — same-cycle read-after-write returns
/// new data.
impl<T: Clone + Default, Domain: ClockDomain> Index<usize> for Memory<T, Domain> {
    type Output = T;

    fn index(&self, index: usize) -> &T {
        self.sync_if_advanced();
        // SAFETY: single-threaded, no mutable alias exists while this shared ref lives.
        let inner = unsafe { &mut *self.inner.get() };
        inner.read_addr = index;
        if self.write_mode == WriteMode::WriteFirst {
            if let Some((wr_addr, ref wr_val)) = inner.staging {
                if wr_addr == index {
                    return wr_val;
                }
            }
        }
        &inner.data[index]
    }
}

/// Queue a write to be committed at the next clock edge. Automatically commits any
/// pending write from the previous cycle if the clock has advanced.
///
/// Only the last write per clock cycle takes effect (single write-port semantics).
/// The staging slot is initialised from the current committed value so that
/// read-modify-write patterns (`mem[i] += 1`) work correctly.
impl<T: Clone + Default, Domain: ClockDomain> IndexMut<usize> for Memory<T, Domain> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.sync_if_advanced();
        // SAFETY: we have &mut self.
        let inner = unsafe { &mut *self.inner.get() };
        inner.staging = Some((index, inner.data[index].clone()));
        &mut inner.staging.as_mut().unwrap().1
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Clock, ClockDomain};

    struct TestClk;
    impl ClockDomain for TestClk {}

    fn clk() -> Clock<TestClk> { Clock::<TestClk>::new() }

    // ── output_reg X semantics ────────────────────────────────────────────────

    #[test]
    fn test_output_is_none_before_first_clock_edge() {
        let clk = clk();
        let mem = Memory::<u8, TestClk>::new(clk.clone(), 4);
        assert_eq!(mem.output(), None);
    }

    #[test]
    fn test_output_some_after_first_clock_edge() {
        let mut clk = clk();
        let mem = Memory::<u8, TestClk>::new(clk.clone(), 4);
        clk.advance();
        assert_eq!(mem.output(), Some(&0u8)); // read_addr defaults to 0, data[0] = 0
    }

    // ── output() triggers sync even in write-only / output-only cycles ────────

    #[test]
    fn test_output_triggers_sync_without_index_call() {
        let mut clk = clk();
        let mem = Memory::<u8, TestClk>::new(clk.clone(), 4);

        clk.advance(); // first edge
        clk.advance(); // second edge — no Index/IndexMut between edges
        // output() alone must trigger sync and return a valid value
        assert_eq!(mem.output(), Some(&0u8));
    }

    #[test]
    fn test_output_correct_in_write_only_cycle() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4);

        let _ = mem[0];   // set read_addr = 0
        clk.advance();    // edge 1: output_reg = Some(0), no write

        mem[0] = 42;      // queue write — IndexMut triggers sync, output_reg = Some(0)
        clk.advance();    // edge 2: write committed, output_reg = Some(data[0]=0) pre-write

        // output() triggers edge-2 sync (read-first: captures 0 before committing 42)
        assert_eq!(mem.output(), Some(&0u8));
        // data[0] is now 42
        assert_eq!(mem[0], 42);
    }

    // ── Sync read-first (default) ─────────────────────────────────────────────

    #[test]
    fn test_sync_read_first_write_visible_next_cycle() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4);

        mem[0] = 42;          // queue write
        let _ = mem[0];       // records read_addr = 0
        clk.advance();
        assert_eq!(mem.output(), Some(&0u8)); // captured BEFORE write (read-first)
        assert_eq!(mem[0], 42);              // data committed
    }

    #[test]
    fn test_sync_read_first_output_reg_updates_each_cycle() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4);

        mem[0] = 10;
        let _ = mem[0];
        clk.advance();
        assert_eq!(mem.output(), Some(&0u8)); // pre-write capture

        let _ = mem[0];
        clk.advance();
        assert_eq!(mem.output(), Some(&10u8)); // now captures committed value
    }

    #[test]
    fn test_sync_no_write_leaves_data_unchanged() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4);
        let _ = mem[0];
        clk.advance();
        assert_eq!(mem[0], 0);
    }

    #[test]
    fn test_read_modify_write() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4);

        mem[0] = 10;
        clk.advance();
        let _ = mem[0]; // trigger commit

        mem[0] += 5;    // staging initialised from committed value 10 → becomes 15
        clk.advance();
        let _ = mem[0]; // trigger commit
        assert_eq!(mem[0], 15);
    }

    // ── Sync write-first ──────────────────────────────────────────────────────

    #[test]
    fn test_sync_write_first_same_cycle_read_sees_new_data() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4).write_first();

        mem[0] = 42;
        assert_eq!(mem[0], 42); // same-cycle read sees pending write
    }

    #[test]
    fn test_sync_write_first_output_reg_includes_write() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4).write_first();

        mem[0] = 42;
        let _ = mem[0]; // read_addr = 0
        clk.advance();
        assert_eq!(mem.output(), Some(&42u8)); // write committed first, then captured
    }

    #[test]
    fn test_sync_write_first_different_address_unaffected() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4).write_first();

        mem[1] = 42;
        assert_eq!(mem[0], 0); // different address, pending write invisible
    }

    // ── Async read ────────────────────────────────────────────────────────────

    #[test]
    fn test_async_read_first_pending_write_invisible() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4).async_read();

        mem[0] = 42;
        assert_eq!(mem[0], 0); // read-first: pending write not visible
        clk.advance();
        assert_eq!(mem[0], 42);
    }

    #[test]
    fn test_async_output_always_none() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4).async_read();
        clk.advance();
        assert_eq!(mem.output(), None); // no output register in async mode
    }

    #[test]
    fn test_async_write_first_pending_write_visible_immediately() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4)
            .async_read()
            .write_first();

        mem[0] = 42;
        assert_eq!(mem[0], 42); // write-first: staging visible immediately
        assert_eq!(mem[1], 0);  // other addresses unaffected
    }

    // ── ROM initialization ────────────────────────────────────────────────────

    #[test]
    fn test_from_contents_preloads_all_entries() {
        let clk = clk();
        let mem = Memory::<u8, TestClk>::from_contents(
            clk.clone(),
            vec![0x11, 0x22, 0x33, 0x44],
        );
        assert_eq!(mem.size(), 4);
        assert_eq!(mem[0], 0x11);
        assert_eq!(mem[1], 0x22);
        assert_eq!(mem[2], 0x33);
        assert_eq!(mem[3], 0x44);
    }

    #[test]
    fn test_from_contents_data_survives_clock_edge() {
        let mut clk = clk();
        let mem = Memory::<u8, TestClk>::from_contents(clk.clone(), vec![0xAA, 0xBB, 0xCC]);
        clk.advance();
        assert_eq!(mem[0], 0xAA);
        assert_eq!(mem[1], 0xBB);
        assert_eq!(mem[2], 0xCC);
    }

    #[test]
    fn test_from_fn_computes_each_entry() {
        let clk = clk();
        let mem = Memory::<u8, TestClk>::from_fn(clk.clone(), 4, |i| (i * 10) as u8);
        assert_eq!(mem[0], 0);
        assert_eq!(mem[1], 10);
        assert_eq!(mem[2], 20);
        assert_eq!(mem[3], 30);
    }

    #[test]
    fn test_from_contents_output_none_before_first_edge() {
        let clk = clk();
        let mem = Memory::<u8, TestClk>::from_contents(clk.clone(), vec![0xFF; 4]);
        // output_reg is still X (None) before the first clock edge, even with preloaded data
        assert_eq!(mem.output(), None);
    }

    // ── Reset ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_reset_makes_output_reg_valid_immediately() {
        let clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4).with_reset(0xFF);
        assert_eq!(mem.output(), None); // still None before reset() is called
        mem.reset();
        assert_eq!(mem.output(), Some(&0xFF)); // reset gives output a valid value
    }

    #[test]
    fn test_reset_cancels_pending_write() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4).with_reset(0u8);
        mem[0] = 42;
        mem.reset();           // cancel pending write
        clk.advance();
        assert_eq!(mem[0], 0); // 42 was never committed
    }

    #[test]
    fn test_reset_noop_without_reset_value() {
        let clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4);
        mem.reset(); // no-op
        assert_eq!(mem.output(), None); // still uninitialized
    }

    // ── Metadata ─────────────────────────────────────────────────────────────

    #[test]
    fn test_size() {
        let clk = clk();
        let mem = Memory::<u8, TestClk>::new(clk.clone(), 1024);
        assert_eq!(mem.size(), 1024);
    }

    #[test]
    fn test_defaults() {
        let clk = clk();
        let mem = Memory::<u8, TestClk>::new(clk.clone(), 4);
        assert_eq!(mem.write_mode(), WriteMode::ReadFirst);
        assert_eq!(mem.read_mode(), ReadMode::Sync);
    }

    // ── Single write-port: last write per cycle wins ──────────────────────────

    #[test]
    fn test_last_write_per_cycle_wins() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4);

        mem[0] = 10;
        mem[0] = 20; // overwrites staging
        clk.advance();
        assert_eq!(mem[0], 20);
    }

    #[test]
    fn test_writes_to_different_addresses_second_wins() {
        let mut clk = clk();
        let mut mem = Memory::<u8, TestClk>::new(clk.clone(), 4);

        mem[0] = 10; // queued
        mem[1] = 20; // overwrites staging — only addr 1 is committed
        clk.advance();
        assert_eq!(mem[0], 0);  // addr 0 write was lost (single write port)
        assert_eq!(mem[1], 20);
    }
}
