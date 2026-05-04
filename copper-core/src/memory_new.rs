use std::cell::UnsafeCell;
use std::ops::{Index, IndexMut};
use crate::types::{Clock, ClockDomain};

/// R and W are the number of read and write ports.
/// Each port is a separate object.
/// Port reuse in the same cycle is tracked per port.
pub struct Memory<T, const R: usize, const W: usize, D: ClockDomain> {
    inner: UnsafeCell<MemoryInner<T, R, W>>,
    clock: Clock<D>,
}

struct MemoryInner<T, const R: usize, const W: usize> {
    data: Vec<T>,
    staging: [Option<(usize, T)>; W],
    read_addr: [usize; R],
    read_valid: [bool; R],
    write_valid: [bool; W],
    output_reg: [Option<T>; R],
    last_cycle: u64,
}

pub struct ReadPort<'a, T, const R: usize, const W: usize, D: ClockDomain, const I: usize> {
    mem: &'a Memory<T, R, W, D>,
}

pub struct WritePort<'a, T, const R: usize, const W: usize, D: ClockDomain, const I: usize> {
    mem: &'a Memory<T, R, W, D>,
}

impl<T: Clone + Default, const R: usize, const W: usize, D: ClockDomain> Memory<T, R, W, D> {
    pub fn new(clock: Clock<D>, size: usize) -> Self {
        Self {
            inner: UnsafeCell::new(MemoryInner {
                data: vec![T::default(); size],
                staging: std::array::from_fn(|_| None),
                read_addr: [0; R],
                read_valid: [false; R],
                write_valid: [false; W],
                output_reg: std::array::from_fn(|_| None),
                last_cycle: clock.cycle(),
            }),
            clock,
        }
    }

    pub fn from_contents(clock: Clock<D>, data: Vec<T>) -> Self {
        Self {
            inner: UnsafeCell::new(MemoryInner {
                data,
                staging: std::array::from_fn(|_| None),
                read_addr: [0; R],
                read_valid: [false; R],
                write_valid: [false; W],
                output_reg: std::array::from_fn(|_| None),
                last_cycle: clock.cycle(),
            }),
            clock,
        }
    }

    pub fn read_port<const I: usize>(&self) -> ReadPort<'_, T, R, W, D, I> {
        ReadPort { mem: self }
    }

    pub fn write_port<const I: usize>(&self) -> WritePort<'_, T, R, W, D, I> {
        WritePort { mem: self }
    }

    fn sync_if_advanced(&self) {
        let cycle = self.clock.cycle();
        // SAFETY: single-threaded simulation executor; UnsafeCell access is single-threaded.
        let inner = unsafe { &mut *self.inner.get() };

        // The clock has not advanced since the last commit, so nothing changes yet.
        if cycle <= inner.last_cycle {
            return;
        }

        // Commit any staged writes on the new clock edge.
        for staged in inner.staging.iter_mut() {
            if let Some((addr, value)) = staged.take() {
                inner.data[addr] = value;
            }
        }

        // Clear per-cycle usage flags for the new cycle.
        inner.read_valid = [false; R];
        inner.write_valid = [false; W];
        inner.last_cycle = cycle;
    }
}

impl<'a, T: Clone, const R: usize, const W: usize, D: ClockDomain, const I: usize>
ReadPort<'a, T, R, W, D, I>
{
    pub fn read(&self, addr: usize) -> Option<T> {
        self.mem.sync_if_advanced();
        // SAFETY: single-threaded simulation executor; UnsafeCell access is single-threaded.
        let inner = unsafe { &mut *self.mem.inner.get() };
        debug_assert!(!inner.read_valid[I], "read port used more than once in a cycle");
        inner.read_valid[I] = true;
        inner.read_addr[I] = addr;
        inner.data[addr].clone()
    }
}

use std::ops::{Index, IndexMut};

impl<'a, T: Clone + Default, const R: usize, const W: usize, D: ClockDomain, const I: usize>
Index<usize> for ReadPort<'a, T, R, W, D, I>
{
    type Output = T;

    fn index(&self, addr: usize) -> &Self::Output {
        // SAFETY: single-threaded simulation executor; UnsafeCell access is single-threaded.
        let inner = unsafe { &mut *self.mem.inner.get() };
        debug_assert!(!inner.read_valid[I], "read port used more than once in a cycle");
        inner.read_valid[I] = true;
        inner.read_addr[I] = addr;

        // Return committed data (write-first staging not reflected here).
        &inner.data[addr]
    }
}

impl<'a, T: Clone + Default, const R: usize, const W: usize, D: ClockDomain, const I: usize>
Index<usize> for &mut ReadPort<'a, T, R, W, D, I>
{
    type Output = T;

    fn index(&self, addr: usize) -> &Self::Output {
        // SAFETY: single-threaded simulation executor; UnsafeCell access is single-threaded.
        let inner = unsafe { &*self.mem.inner.get() };
        &inner.data[addr]
    }
}


impl<'a, T: Clone, const R: usize, const W: usize, D: ClockDomain, const I: usize>
WritePort<'a, T, R, W, D, I>
{
    pub fn write(&self, addr: usize, value: T) {
        self.mem.sync_if_advanced();

        // SAFETY: single-threaded simulation executor; UnsafeCell access is single-threaded.
        let inner = unsafe { &mut *self.mem.inner.get() };
        debug_assert!(!inner.write_valid[I], "write port used more than once in a cycle");
        inner.write_valid[I] = true;
        inner.staging[I] = Some((addr, value));
    }
}

impl<'a, T: Clone + Default, const R: usize, const W: usize, D: ClockDomain, const I: usize>
IndexMut<usize> for WritePort<'a, T, R, W, D, I>
{
    fn index_mut(&mut self, addr: usize) -> &mut Self::Output {
        self.mem.sync_if_advanced();
        // SAFETY: single-threaded simulation executor; UnsafeCell access is single-threaded.
        let inner = unsafe { &mut *self.mem.inner.get() };
        debug_assert!(!inner.write_valid[I], "write port used more than once in a cycle");
        inner.write_valid[I] = true;
        
        // Initialize staging with current data value for read-modify-write
        inner.staging[I] = Some((addr, inner.data[addr].clone()));
        
        // Return mutable reference to the staged value
        &mut inner.staging[I].as_mut().unwrap().1
    }
}

impl<'a, T: Clone + Default, const R: usize, const W: usize, D: ClockDomain, const I: usize>
Index<usize> for WritePort<'a, T, R, W, D, I>
{
    type Output = T;

    fn index(&self, addr: usize) -> &Self::Output {
        // For a write port, Index returns the staged value if it exists at this addr,
        // otherwise the committed value (write-first semantics)
        // Return committed data; use `index_mut` to stage writes.
        // SAFETY: single-threaded simulation executor; UnsafeCell access is single-threaded.
        let inner = unsafe { &*self.mem.inner.get() };
        &inner.data[addr]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Clock, ClockDomain};

    struct TestClk;
    impl ClockDomain for TestClk {}

    fn clk() -> Clock<TestClk> {
        Clock::<TestClk>::new()
    }

    #[test]
    fn from_contents_preloads_data() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::from_contents(clock, vec![0x11, 0x22, 0x33]);

        let read_port = memory.read_port::<0>();
        assert_eq!(read_port.read(0), Some(0x11));
        assert_eq!(read_port.read(1), Some(0x22));
        assert_eq!(read_port.read(2), Some(0x33));
    }

    #[test]
    fn write_commits_on_next_clock_edge() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4);
        let write_port = memory.write_port::<0>();
        let read_port = memory.read_port::<0>();

        write_port.write(2, 0xAB);
        assert_eq!(read_port.read(2), Some(0));

        clock.advance();
        assert_eq!(read_port.read(2), Some(0xAB));
    }

    #[test]
    fn last_write_in_same_cycle_wins() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4);
        let write_port = memory.write_port::<0>();
        let read_port = memory.read_port::<0>();

        write_port.write(1, 0x10);
        write_port.write(1, 0x20);

        clock.advance();
        assert_eq!(read_port.read(1), Some(0x20));
    }

    #[test]
    fn different_read_ports_can_be_used_in_same_cycle() {
        let clock = clk();
        let memory = Memory::<u8, 2, 1, TestClk>::from_contents(clock, vec![1, 2, 3, 4]);
        let read0 = memory.read_port::<0>();
        let read1 = memory.read_port::<1>();

        assert_eq!(read0.read(0), Some(1));
        assert_eq!(read1.read(3), Some(4));
    }

    #[test]
    #[should_panic(expected = "read port used more than once in a cycle")]
    fn reusing_same_read_port_in_same_cycle_panics() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::from_contents(clock, vec![9]);
        let read_port = memory.read_port::<0>();

        assert_eq!(read_port.read(0), Some(9));
        let _ = read_port.read(0);
    }

    #[test]
    fn read_port_index_mut_works() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::from_contents(clock, vec![0xAA, 0xBB, 0xCC]);
        let mut read_port = memory.read_port::<0>();

        // Use array-index syntax; Index returns &T, dereference to get value
        assert_eq!(*read_port[0], 0xAA);
        assert_eq!(*read_port[1], 0xBB);
        assert_eq!(*read_port[2], 0xCC);
    }

    #[test]
    fn write_port_index_mut_stages_write() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::new(clock.clone(), 4);
        let mut write_port = memory.write_port::<0>();
        let read_port = memory.read_port::<0>();

        write_port[3] = 0x42;
        assert_eq!(read_port.read(3), Some(0));

        clock.advance();
        assert_eq!(read_port.read(3), Some(0x42));
    }

    #[test]
    fn write_port_index_mut_read_modify_write() {
        let mut clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::from_contents(clock.clone(), vec![10, 20, 30]);
        let mut write_port = memory.write_port::<0>();

        write_port[1] += 5;
        clock.advance();

        let read_port = memory.read_port::<0>();
        assert_eq!(read_port.read(1), Some(25));
    }

    #[test]
    #[should_panic(expected = "write port used more than once in a cycle")]
    fn write_port_index_mut_reuse_panics() {
        let clock = clk();
        let memory = Memory::<u8, 1, 1, TestClk>::from_contents(clock, vec![1, 2, 3]);
        let mut write_port = memory.write_port::<0>();

        write_port[0] = 100;
        let _ = &mut write_port[1];
    }
}
