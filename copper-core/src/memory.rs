use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::types::{Clock, ClockDomain, ClockEdgeListener};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    ReadFirst,
    WriteFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    Sync,
    Async,
}

/// Multi-port synchronous memory with configurable read and write latency.
///
/// - `R` = read port count, `W` = write port count
/// - `READ_LAT` = cycles from address presentation to valid output (≥ 1)
/// - `WRITE_LAT` = cycles from write call to data committed in memory (≥ 1)
///
/// Both read and write ports are fully pipelined: a new address/value may be
/// presented every cycle regardless of in-flight accesses. `ReadPort::is_ready`
/// signals when the output pipeline holds valid data. `WritePort::is_ready` is
/// always true (pipelined writes are always accepted).
pub struct Memory<
    T,
    const R: usize,
    const W: usize,
    D: ClockDomain,
    const READ_LAT: usize = 1,
    const WRITE_LAT: usize = 1,
> {
    shared: Arc<MemoryShared<T, R, W, READ_LAT, WRITE_LAT>>,
    _clock: Clock<D>,
    read_mode: ReadMode,
}

struct MemoryInner<T, const R: usize, const W: usize, const READ_LAT: usize, const WRITE_LAT: usize>
{
    data: Vec<T>,
    /// Per-write-port pipeline. Index [port][0] is the most recently issued
    /// write; index [port][WRITE_LAT-1] commits on the next posedge.
    write_pipeline: [[Option<(usize, T)>; WRITE_LAT]; W],
    /// Per-read-port pipeline. Index [port][0] is the value captured at the
    /// most recent posedge; index [port][READ_LAT-1] is the output.
    read_pipeline: [[Option<T>; READ_LAT]; R],
    /// Address staged by the user this cycle, captured into the pipeline on
    /// posedge. Cleared after each capture so the user must re-present the
    /// address every cycle to keep the pipeline active.
    pending_read_addr: [Option<usize>; R],
}

struct MemoryShared<T, const R: usize, const W: usize, const READ_LAT: usize, const WRITE_LAT: usize>
{
    inner: Mutex<MemoryInner<T, R, W, READ_LAT, WRITE_LAT>>,
    /// `true` = WriteFirst, `false` = ReadFirst. Stored atomically so the
    /// builder methods (`write_first` / `read_first`) can set it through the
    /// shared `Arc` without needing exclusive ownership.
    write_first_mode: AtomicBool,
}

impl<
    T: Clone + Send + Sync,
    const R: usize,
    const W: usize,
    const READ_LAT: usize,
    const WRITE_LAT: usize,
> ClockEdgeListener for MemoryShared<T, R, W, READ_LAT, WRITE_LAT>
{
    fn on_posedge(&self) {
        let mut inner = self.inner.lock().unwrap();

        let write_first = self.write_first_mode.load(Ordering::Relaxed);
        match write_first {
            false => {
                // ReadFirst: capture reads before committing writes so a
                // simultaneous read/write at the same address sees the old value.
                advance_read_pipelines::<T, R, W, READ_LAT, WRITE_LAT>(&mut inner);
                advance_write_pipelines::<T, R, W, READ_LAT, WRITE_LAT>(&mut inner);
            }
            true => {
                // WriteFirst: commit writes before capturing reads so a
                // simultaneous read/write at the same address sees the new value.
                advance_write_pipelines::<T, R, W, READ_LAT, WRITE_LAT>(&mut inner);
                advance_read_pipelines::<T, R, W, READ_LAT, WRITE_LAT>(&mut inner);
            }
        }
    }
}

fn advance_write_pipelines<
    T,
    const R: usize,
    const W: usize,
    const READ_LAT: usize,
    const WRITE_LAT: usize,
>(
    inner: &mut MemoryInner<T, R, W, READ_LAT, WRITE_LAT>,
) {
    for port in 0..W {
        // Commit the oldest stage.
        if let Some((addr, value)) = inner.write_pipeline[port][WRITE_LAT - 1].take() {
            inner.data[addr] = value;
        }
        // Shift toward the output end: stage N ← stage N-1.
        for stage in (1..WRITE_LAT).rev() {
            inner.write_pipeline[port][stage] = inner.write_pipeline[port][stage - 1].take();
        }
        // Stage 0 is now empty, ready for the next write this cycle.
    }
}

fn advance_read_pipelines<
    T: Clone,
    const R: usize,
    const W: usize,
    const READ_LAT: usize,
    const WRITE_LAT: usize,
>(
    inner: &mut MemoryInner<T, R, W, READ_LAT, WRITE_LAT>,
) {
    for port in 0..R {
        // Shift toward the output end: stage N ← stage N-1.
        for stage in (1..READ_LAT).rev() {
            inner.read_pipeline[port][stage] = inner.read_pipeline[port][stage - 1].take();
        }
        // Capture pending address into stage 0; clear pending so it must be
        // re-presented next cycle.
        inner.read_pipeline[port][0] = inner.pending_read_addr[port]
            .take()
            .map(|addr| inner.data[addr].clone());
    }
}

// ── Port handles ──────────────────────────────────────────────────────────────

pub struct ReadPort<
    'a,
    T,
    const R: usize,
    const W: usize,
    D: ClockDomain,
    const I: usize,
    const READ_LAT: usize,
    const WRITE_LAT: usize,
> {
    mem: &'a Memory<T, R, W, D, READ_LAT, WRITE_LAT>,
}

pub struct WritePort<
    'a,
    T,
    const R: usize,
    const W: usize,
    D: ClockDomain,
    const I: usize,
    const READ_LAT: usize,
    const WRITE_LAT: usize,
> {
    mem: &'a Memory<T, R, W, D, READ_LAT, WRITE_LAT>,
}

// ── Memory methods ────────────────────────────────────────────────────────────

impl<T, const R: usize, const W: usize, D: ClockDomain, const READ_LAT: usize, const WRITE_LAT: usize>
    Memory<T, R, W, D, READ_LAT, WRITE_LAT>
{
    pub fn read_port<const I: usize>(&self) -> ReadPort<'_, T, R, W, D, I, READ_LAT, WRITE_LAT> {
        ReadPort { mem: self }
    }

    pub fn write_port<const I: usize>(&self) -> WritePort<'_, T, R, W, D, I, READ_LAT, WRITE_LAT> {
        WritePort { mem: self }
    }

    pub fn size(&self) -> usize {
        self.shared.inner.lock().unwrap().data.len()
    }

    pub fn write_mode(&self) -> WriteMode {
        if self.shared.write_first_mode.load(Ordering::Relaxed) {
            WriteMode::WriteFirst
        } else {
            WriteMode::ReadFirst
        }
    }

    pub fn read_mode(&self) -> ReadMode {
        self.read_mode
    }
}

impl<
    T: Default + Clone + Send + Sync + 'static,
    const R: usize,
    const W: usize,
    D: ClockDomain,
    const READ_LAT: usize,
    const WRITE_LAT: usize,
> Memory<T, R, W, D, READ_LAT, WRITE_LAT>
{
    pub fn new(clock: Clock<D>, size: usize) -> Self {
        let data = std::iter::repeat_with(T::default).take(size).collect();
        Self::from_data(clock, data)
    }
}

impl<
    T: Clone + Send + Sync + 'static,
    const R: usize,
    const W: usize,
    D: ClockDomain,
    const READ_LAT: usize,
    const WRITE_LAT: usize,
> Memory<T, R, W, D, READ_LAT, WRITE_LAT>
{
    pub fn from_contents(clock: Clock<D>, data: Vec<T>) -> Self {
        Self::from_data(clock, data)
    }

    pub fn from_fn(clock: Clock<D>, size: usize, mut f: impl FnMut(usize) -> T) -> Self {
        let data = (0..size).map(|i| f(i)).collect();
        Self::from_data(clock, data)
    }

    fn from_data(clock: Clock<D>, data: Vec<T>) -> Self {
        let shared = Arc::new(MemoryShared {
            inner: Mutex::new(MemoryInner {
                data,
                write_pipeline: std::array::from_fn(|_| std::array::from_fn(|_| None)),
                read_pipeline: std::array::from_fn(|_| std::array::from_fn(|_| None)),
                pending_read_addr: std::array::from_fn(|_| None),
            }),
            write_first_mode: AtomicBool::new(false),
        });
        let listener: Arc<dyn ClockEdgeListener> = shared.clone();
        clock.register_listener(Arc::downgrade(&listener));
        Self {
            shared,
            _clock: clock,
            read_mode: ReadMode::Sync,
        }
    }
}

// ── ReadPort methods ──────────────────────────────────────────────────────────

impl<
    'a,
    T: Clone,
    const R: usize,
    const W: usize,
    D: ClockDomain,
    const I: usize,
    const READ_LAT: usize,
    const WRITE_LAT: usize,
> ReadPort<'a, T, R, W, D, I, READ_LAT, WRITE_LAT>
{
    /// Stage `addr` to be captured into the read pipeline on the next posedge.
    ///
    /// After `READ_LAT` posedges, `data()` will return the value at `addr`.
    /// Call this every cycle to keep the pipeline continuously fed.
    pub fn read(&self, addr: usize) {
        self.mem.shared.inner.lock().unwrap().pending_read_addr[I] = Some(addr);
    }

    /// Returns `true` once the output stage of the read pipeline holds valid
    /// data (i.e., at least `READ_LAT` posedges have occurred since the first
    /// `read()` call on this port).
    pub fn is_ready(&self) -> bool {
        self.mem.shared.inner.lock().unwrap().read_pipeline[I][READ_LAT - 1].is_some()
    }

    /// Returns the value at the read pipeline output.
    ///
    /// Panics if `is_ready()` is `false`.
    pub fn data(&self) -> T {
        self.mem.shared.inner.lock().unwrap().read_pipeline[I][READ_LAT - 1]
            .clone()
            .expect("read pipeline output is not yet valid; check is_ready() first")
    }
}

// ── Memory builder methods ────────────────────────────────────────────────────

impl<T, const R: usize, const W: usize, D: ClockDomain, const READ_LAT: usize, const WRITE_LAT: usize>
    Memory<T, R, W, D, READ_LAT, WRITE_LAT>
{
    /// Set WriteFirst read-during-write mode. Returns self for chaining.
    ///
    /// Called by the `#[hardware]` macro when it infers WriteFirst ordering from
    /// the position of reads and writes in the loop body.
    pub fn write_first(self) -> Self {
        self.shared.write_first_mode.store(true, Ordering::Relaxed);
        self
    }

    /// Set ReadFirst read-during-write mode (the default). Returns self for chaining.
    pub fn read_first(self) -> Self {
        self.shared.write_first_mode.store(false, Ordering::Relaxed);
        self
    }
}

// ── WritePort methods ─────────────────────────────────────────────────────────

impl<
    'a,
    T,
    const R: usize,
    const W: usize,
    D: ClockDomain,
    const I: usize,
    const READ_LAT: usize,
    const WRITE_LAT: usize,
> WritePort<'a, T, R, W, D, I, READ_LAT, WRITE_LAT>
{
    /// Stage a write into the write pipeline. The value will be committed to
    /// memory after `WRITE_LAT` posedges.
    pub fn write(&self, addr: usize, value: T) {
        self.mem.shared.inner.lock().unwrap().write_pipeline[I][0] = Some((addr, value));
    }

    /// Always returns `true`: pipelined write ports accept a new write every
    /// cycle regardless of in-flight writes.
    pub fn is_ready(&self) -> bool {
        true
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

    // ── Read latency ──────────────────────────────────────────────────────────

    #[test]
    fn read_lat1_not_ready_before_first_tick() {
        let clk = clk();
        let mem = Memory::<u8, 1, 0, TestClk>::from_contents(clk, vec![0xAA, 0xBB]);
        mem.read_port::<0>().read(0);
        assert!(!mem.read_port::<0>().is_ready());
    }

    #[test]
    fn read_lat1_ready_after_one_tick() {
        let mut clk = clk();
        let mem = Memory::<u8, 1, 0, TestClk>::from_contents(clk.clone(), vec![0x42]);
        mem.read_port::<0>().read(0);
        clk.advance();
        assert!(mem.read_port::<0>().is_ready());
        assert_eq!(mem.read_port::<0>().data(), 0x42);
    }

    #[test]
    fn read_lat2_not_ready_after_one_tick() {
        let mut clk = clk();
        let mem = Memory::<u8, 1, 0, TestClk, 2, 1>::from_contents(clk.clone(), vec![0x99]);
        mem.read_port::<0>().read(0);
        clk.advance();
        assert!(!mem.read_port::<0>().is_ready());
    }

    #[test]
    fn read_lat2_ready_after_two_ticks() {
        let mut clk = clk();
        let mem = Memory::<u8, 1, 0, TestClk, 2, 1>::from_contents(clk.clone(), vec![0x77]);
        // Cycle 0: stage address
        mem.read_port::<0>().read(0);
        clk.advance(); // posedge 0: captured into pipeline[0]
        // Cycle 1: re-stage (keep pipeline fed); still not ready
        mem.read_port::<0>().read(0);
        assert!(!mem.read_port::<0>().is_ready());
        clk.advance(); // posedge 1: pipeline[1] = pipeline[0] → output valid
        assert!(mem.read_port::<0>().is_ready());
        assert_eq!(mem.read_port::<0>().data(), 0x77);
    }

    #[test]
    fn pipelined_reads_deliver_correct_values_in_order() {
        // Two different addresses staged in consecutive cycles; verify that
        // each result appears in the correct slot as the pipeline shifts.
        let mut clk = clk();
        let mem = Memory::<u8, 1, 0, TestClk, 2, 1>::from_contents(
            clk.clone(),
            vec![0x11, 0x22, 0x33],
        );

        // Cycle 0: stage addr 0
        mem.read_port::<0>().read(0);
        clk.advance();

        // Cycle 1: stage addr 1; addr-0 result not yet at output
        mem.read_port::<0>().read(1);
        assert!(!mem.read_port::<0>().is_ready());
        clk.advance();

        // Cycle 2: addr-0 result arrives at output (READ_LAT=2)
        assert!(mem.read_port::<0>().is_ready());
        assert_eq!(mem.read_port::<0>().data(), 0x11);

        // Stage addr 2 and advance; addr-1 result arrives next
        mem.read_port::<0>().read(2);
        clk.advance();

        assert_eq!(mem.read_port::<0>().data(), 0x22);

        mem.read_port::<0>().read(0); // any address to keep pipeline fed
        clk.advance();

        assert_eq!(mem.read_port::<0>().data(), 0x33);
    }

    // ── Write latency ─────────────────────────────────────────────────────────

    #[test]
    fn write_lat1_commits_after_one_tick() {
        let mut clk = clk();
        let mem = Memory::<u8, 1, 1, TestClk>::new(clk.clone(), 4);

        mem.write_port::<0>().write(2, 0xCD);
        // Not visible yet — write is in flight
        mem.read_port::<0>().read(2);
        clk.advance(); // posedge: ReadFirst → read captures 0, write commits 0xCD

        // Pipeline output holds pre-write value (ReadFirst default)
        assert_eq!(mem.read_port::<0>().data(), 0x00);

        // Next tick: addr 2 staged again, data[2] is now 0xCD
        mem.read_port::<0>().read(2);
        clk.advance();
        assert_eq!(mem.read_port::<0>().data(), 0xCD);
    }

    #[test]
    fn write_lat2_commits_after_two_ticks() {
        let mut clk = clk();
        let mem = Memory::<u8, 1, 1, TestClk, 1, 2>::new(clk.clone(), 4);

        // Stage write; it takes 2 posedges to commit.
        mem.write_port::<0>().write(3, 0xEF);
        clk.advance(); // posedge 0: write moves to pipeline stage[1]

        // Posedge 1: ReadFirst — read captures data[3] BEFORE the write commits,
        // so it still sees 0. Write commits at the end of this posedge.
        mem.read_port::<0>().read(3);
        clk.advance();
        assert_eq!(mem.read_port::<0>().data(), 0x00);

        // Posedge 2: write has already committed at posedge 1; no concurrent
        // write this cycle, so the read captures the committed value 0xEF.
        mem.read_port::<0>().read(3);
        clk.advance();
        assert_eq!(mem.read_port::<0>().data(), 0xEF);
    }

    #[test]
    fn write_port_is_ready_is_always_true() {
        let clk = clk();
        let mem = Memory::<u8, 0, 1, TestClk>::new(clk, 4);
        assert!(mem.write_port::<0>().is_ready());
    }

    // ── Read-during-write semantics ───────────────────────────────────────────

    #[test]
    fn read_first_sees_old_value_on_same_addr_write() {
        let mut clk = clk();
        // data[0] starts at 0
        let mem = Memory::<u8, 1, 1, TestClk>::new(clk.clone(), 4);

        // Same cycle: read and write addr 0
        mem.read_port::<0>().read(0);
        mem.write_port::<0>().write(0, 0xFF);
        clk.advance(); // ReadFirst: read captures 0 before write commits

        assert_eq!(mem.read_port::<0>().data(), 0x00);
    }

    #[test]
    fn write_first_sees_new_value_on_same_addr_write() {
        let mut clk = clk();
        let mem = Memory::<u8, 1, 1, TestClk>::new(clk.clone(), 4).write_first();

        // Same cycle: read and write addr 0
        mem.read_port::<0>().read(0);
        mem.write_port::<0>().write(0, 0xFF);
        clk.advance(); // WriteFirst: write commits before read captures

        assert_eq!(mem.read_port::<0>().data(), 0xFF);
    }

    #[test]
    fn read_first_sees_old_value_different_addr_unaffected() {
        let mut clk = clk();
        let mem = Memory::<u8, 1, 1, TestClk>::from_contents(clk.clone(), vec![0x10, 0x20]);

        // Write addr 1, read addr 0 simultaneously
        mem.read_port::<0>().read(0);
        mem.write_port::<0>().write(1, 0xFF);
        clk.advance();

        // Read at addr 0 is unaffected by write at addr 1
        assert_eq!(mem.read_port::<0>().data(), 0x10);
    }

    // ── Multiple ports ────────────────────────────────────────────────────────

    #[test]
    fn two_read_ports_independent() {
        let mut clk = clk();
        let mem = Memory::<u8, 2, 0, TestClk>::from_contents(clk.clone(), vec![0xAA, 0xBB]);

        mem.read_port::<0>().read(0);
        mem.read_port::<1>().read(1);
        clk.advance();

        assert_eq!(mem.read_port::<0>().data(), 0xAA);
        assert_eq!(mem.read_port::<1>().data(), 0xBB);
    }

    #[test]
    fn two_write_ports_last_wins_at_same_addr() {
        // When both write ports target the same address in the same cycle,
        // port 0 commits first (lower index), then port 1 overwrites it.
        let mut clk = clk();
        let mem = Memory::<u8, 1, 2, TestClk>::new(clk.clone(), 4);

        mem.write_port::<0>().write(0, 0x11);
        mem.write_port::<1>().write(0, 0x22);
        clk.advance(); // both commit: port 0 then port 1 → 0x22 wins

        mem.read_port::<0>().read(0);
        clk.advance();
        assert_eq!(mem.read_port::<0>().data(), 0x22);
    }

    // ── Constructors ──────────────────────────────────────────────────────────

    #[test]
    fn from_contents_preloads_data() {
        let mut clk = clk();
        let mem = Memory::<u32, 1, 0, TestClk>::from_contents(clk.clone(), vec![10, 20, 30]);
        assert_eq!(mem.size(), 3);
        mem.read_port::<0>().read(2);
        clk.advance();
        assert_eq!(mem.read_port::<0>().data(), 30u32);
    }

    #[test]
    fn from_fn_preloads_data() {
        let mut clk = clk();
        let mem = Memory::<u32, 1, 0, TestClk>::from_fn(clk.clone(), 4, |i| (i * i) as u32);
        mem.read_port::<0>().read(3);
        clk.advance();
        assert_eq!(mem.read_port::<0>().data(), 9u32);
    }

    #[test]
    fn new_initializes_to_default() {
        let mut clk = clk();
        let mem = Memory::<u8, 1, 0, TestClk>::new(clk.clone(), 8);
        mem.read_port::<0>().read(7);
        clk.advance();
        assert_eq!(mem.read_port::<0>().data(), 0u8);
    }
}
