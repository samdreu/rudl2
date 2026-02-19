//! Core type system for Copper HDL
//! 
//! This module defines the foundational types for hardware design:
//! - `Bit`: Single bit with 4-state logic
//! - `Bits<N>`: Bit vectors of compile-time width
//! - `Signal<Domain, T>`: Clock-domain-tagged signals
//! - `Clock`: Clock source for synchronous logic
//! - `State<T>`: Wrapper for sequential state

use std::marker::PhantomData;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::task::Waker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]

// primitive logic values (0, 1, X)
pub enum Logic {
    Zero = 0,
    One = 1,
    X, // unknown
}

/// A single hardware bit with 4-state logic (0, 1, X, Z)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bit(pub Logic);

impl Bit {
    /// Constant for logic zero
    pub const ZERO: Bit = Bit(Logic::Zero);
    
    /// Constant for logic one
    pub const ONE: Bit = Bit(Logic::One);
    
    /// Constant for unknown/uninitialized
    pub const X: Bit = Bit(Logic::X);
    
    /// Create a new bit from a boolean
    pub fn from_bool(b: bool) -> Self {
        if b { Self::ONE } else { Self::ZERO }
    }
    
    /// Convert to boolean if possible (panics on X/Z)
    pub fn as_bool(&self) -> bool {
        match self.0 {
            Logic::Zero => false,
            Logic::One => true,
            Logic::X => panic!("Cannot convert X to bool"),
        }
    }
    
    /// Check if this bit is a valid boolean (not X)
    pub fn is_valid(&self) -> bool {
        matches!(self.0, Logic::Zero | Logic::One)
    }
}

impl From<bool> for Bit {
    fn from(b: bool) -> Self {
        Self::from_bool(b)
    }
}

impl From<Logic> for Bit {
    fn from(l: Logic) -> Self {
        Bit(l)
    }
}

impl std::ops::Not for Bit {
    type Output = Bit;
    
    fn not(self) -> Self::Output {
        match self.0 {
            Logic::Zero => Bit::ONE,
            Logic::One => Bit::ZERO,
            Logic::X => Bit::X,
        }
    }
}

impl std::ops::BitAnd for Bit {
    type Output = Bit;
    
    fn bitand(self, rhs: Self) -> Self::Output {
        match (self.0, rhs.0) {
            (Logic::Zero, _) | (_, Logic::Zero) => Bit::ZERO,
            (Logic::One, Logic::One) => Bit::ONE,
            _ => Bit::X,
        }
    }
}

impl std::ops::BitOr for Bit {
    type Output = Bit;
    
    fn bitor(self, rhs: Self) -> Self::Output {
        match (self.0, rhs.0) {
            (Logic::One, _) | (_, Logic::One) => Bit::ONE,
            (Logic::Zero, Logic::Zero) => Bit::ZERO,
            _ => Bit::X,
        }
    }
}

impl std::ops::BitXor for Bit {
    type Output = Bit;
    
    fn bitxor(self, rhs: Self) -> Self::Output {
        match (self.0, rhs.0) {
            (Logic::Zero, Logic::Zero) | (Logic::One, Logic::One) => Bit::ZERO,
            (Logic::Zero, Logic::One) | (Logic::One, Logic::Zero) => Bit::ONE,
            _ => Bit::X,
        }
    }
}

impl fmt::Display for Bit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Logic::Zero => write!(f, "0"),
            Logic::One => write!(f, "1"),
            Logic::X => write!(f, "X"),
        }
    }
}

/// A bit vector of compile-time constant width N
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Bits<const N: usize> {
    bits: [Logic; N],
}

impl<const N: usize> Bits<N> {
    /// Create a bit vector with all zeros
    pub fn zero() -> Self {
        Self { bits: [Logic::Zero; N] }
    }
    
    /// Create a bit vector with all ones
    pub fn ones() -> Self {
        Self { bits: [Logic::One; N] }
    }
    
    /// Create a bit vector with all X (unknown)
    pub fn x() -> Self {
        Self { bits: [Logic::X; N] }
    }
    
    /// Create from an array of Logic values
    pub fn from_array(bits: [Logic; N]) -> Self {
        Self { bits }
    }
    
    /// Create from a slice (panics if length doesn't match N)
    pub fn from_slice(slice: &[Logic]) -> Self {
        assert_eq!(slice.len(), N, "Slice length must match N");
        let mut bits = [Logic::Zero; N];
        bits.copy_from_slice(slice);
        Self { bits }
    }
    
    /// Create from an unsigned integer (up to u128)
    pub fn from_u128(val: u128) -> Self {
        let mut bits = [Logic::Zero; N];
        for i in 0..N {
            bits[i] = if (val >> i) & 1 == 1 {
                Logic::One
            } else {
                Logic::Zero
            };
        }
        Self { bits }
    }
    
    /// Convert to u128 (panics if any bit is X)
    pub fn as_u128(&self) -> u128 {
        let mut result = 0u128;
        for (i, bit) in self.bits.iter().enumerate() {
            match bit {
                Logic::One => result |= 1 << i,
                Logic::Zero => {},
                Logic::X => panic!("Cannot convert X to integer"),
            }
        }
        result
    }
    
    /// Get the bit at index i (LSB = 0)
    pub fn get(&self, i: usize) -> Bit {
        assert!(i < N, "Bit index out of bounds");
        Bit(self.bits[i])
    }
    
    /// Set the bit at index i
    pub fn set(&mut self, i: usize, bit: Bit) {
        assert!(i < N, "Bit index out of bounds");
        self.bits[i] = bit.0;
    }
    
    /// Get the internal array
    pub fn as_array(&self) -> &[Logic; N] {
        &self.bits
    }
    
    /// Get mutable internal array
    pub fn as_array_mut(&mut self) -> &mut [Logic; N] {
        &mut self.bits
    }
    
    /// Check if all bits are valid (not X)
    pub fn is_valid(&self) -> bool {
        self.bits.iter().all(|b| matches!(b, Logic::Zero | Logic::One))
    }
    
    /// Shift left by n positions (logical shift)
    pub fn shift_left(&self, n: usize) -> Self {
        let mut result = [Logic::Zero; N];
        for i in n..N {
            result[i] = self.bits[i - n];
        }
        Self { bits: result }
    }
    
    /// Shift right by n positions (logical shift)
    pub fn shift_right(&self, n: usize) -> Self {
        let mut result = [Logic::Zero; N];
        for i in 0..(N.saturating_sub(n)) {
            result[i] = self.bits[i + n];
        }
        Self { bits: result }
    }
    
    /// Set the LSB (bit 0) to a new value
    pub fn with_lsb(&self, bit: Bit) -> Self {
        let mut result = self.clone();
        result.bits[0] = bit.0;
        result
    }
    
    /// Set the MSB (bit N-1) to a new value
    pub fn with_msb(&self, bit: Bit) -> Self {
        let mut result = self.clone();
        result.bits[N - 1] = bit.0;
        result
    }
}

impl<const N: usize> Default for Bits<N> {
    fn default() -> Self {
        Self::zero()
    }
}

impl<const N: usize> fmt::Debug for Bits<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bits<{}>(", N)?;
        for (i, bit) in self.bits.iter().enumerate().rev() {
            match bit {
                Logic::Zero => write!(f, "0")?,
                Logic::One => write!(f, "1")?,
                Logic::X => write!(f, "X")?,
            }
            if i > 0 && i % 4 == 0 {
                write!(f, "_")?;
            }
        }
        write!(f, ")")
    }
}

impl<const N: usize> fmt::Display for Bits<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for bit in self.bits.iter().rev() {
            match bit {
                Logic::Zero => write!(f, "0")?,
                Logic::One => write!(f, "1")?,
                Logic::X => write!(f, "X")?,
            }
        }
        Ok(())
    }
}

// Arithmetic operations
impl<const N: usize> std::ops::Add for Bits<N> {
    type Output = Self;
    
    fn add(self, rhs: Self) -> Self::Output {
        let a = self.as_u128();
        let b = rhs.as_u128();
        let sum = a.wrapping_add(b);
        Self::from_u128(sum)
    }
}

impl<const N: usize> std::ops::Not for Bits<N> {
    type Output = Self;
    
    fn not(self) -> Self::Output {
        let mut result = [Logic::Zero; N];
        for i in 0..N {
            result[i] = match self.bits[i] {
                Logic::Zero => Logic::One,
                Logic::One => Logic::Zero,
                Logic::X => Logic::X,
            };
        }
        Self { bits: result }
    }
}

/// Clock domain marker (phantom type for compile-time tracking)
/// 
/// This trait marks types that represent clock domains.
/// Users create their own clock domain types and implement this trait.
/// 
/// # Example
/// ```
/// use copper_core::ClockDomain;
/// 
/// struct ClkMain;
/// impl ClockDomain for ClkMain {}
/// 
/// struct ClkPeripheral;
/// impl ClockDomain for ClkPeripheral {}
/// ```
pub trait ClockDomain: 'static {}

/// A signal tagged with a clock domain
/// 
/// The type system ensures that signals from different clock domains
/// cannot be mixed without explicit synchronization.
/// 
/// # Type Parameters
/// - `Domain`: The clock domain (phantom type implementing `ClockDomain`)
/// - `T`: The signal type (typically `Bit` or `Bits<N>`)
pub struct Signal<Domain: ClockDomain, T> {
    value: T,
    _domain: PhantomData<Domain>,
}

impl<Domain: ClockDomain, T> Signal<Domain, T> {
    /// Create a new signal in the given domain
    pub fn new(value: T) -> Self {
        Self {
            value,
            _domain: PhantomData,
        }
    }
    
    /// Read the signal value (only within the same domain)
    pub fn read(&self) -> &T {
        &self.value
    }
    
    /// Write a new value to the signal
    pub fn write(&mut self, value: T) {
        self.value = value;
    }
    
    /// Get a mutable reference to the value
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<Domain: ClockDomain, T: Clone> Signal<Domain, T> {
    /// Clone the value (not the signal itself)
    pub fn value_clone(&self) -> T {
        self.value.clone()
    }
}

impl<Domain: ClockDomain, T: Clone> Clone for Signal<Domain, T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            _domain: PhantomData,
        }
    }
}

impl<Domain: ClockDomain, T: fmt::Debug> fmt::Debug for Signal<Domain, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signal<{}>(", std::any::type_name::<Domain>())?;
        self.value.fmt(f)?;
        write!(f, ")")
    }
}

#[derive(Debug)]
struct ClockState {
    cycle: u64,
    wakers: Vec<Waker>,
}

/// Clock source for synchronous logic
/// 
/// Represents a clock signal that can be awaited in async state machines.
/// Each clock has an associated domain type for safety.
#[derive(Debug)]
pub struct Clock<Domain: ClockDomain> {
    state: Arc<Mutex<ClockState>>,
    _domain: PhantomData<Domain>,
}

impl<Domain: ClockDomain> Clock<Domain> {
    /// Create a new clock starting at cycle 0
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ClockState {
                cycle: 0,
                wakers: Vec::new(),
            })),
            _domain: PhantomData,
        }
    }
    
    /// Get the current cycle number
    pub fn cycle(&self) -> u64 {
        self.state.lock().unwrap().cycle
    }
    
    /// Advance the clock by one cycle (for simulation)
    pub fn advance(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.cycle += 1;
        // wake any tasks waiting on this clock tick
        let wakers = std::mem::take(&mut state.wakers);
        drop(state); // release lock before waking ?????
        for w in wakers {
            w.wake();
        }
    }
    
    /// Create a future that completes on the next clock edge
    /// 
    /// This is intended to be used with `.await` in async hardware functions:
    /// ```ignore
    /// async fn counter(clk: Clock<MainClk>) {
    ///     loop {
    ///         clk.tick().await;
    ///         // ... state transitions
    ///     }
    /// }
    /// ```
    pub fn tick(&self) -> ClockTick<Domain> {
        let target = self.cycle().wrapping_add(1); 
        ClockTick {
            state: Arc::clone(&self.state),
            target_cycle: target,
            _domain: PhantomData,
        }
    }
}

impl<Domain: ClockDomain> Default for Clock<Domain> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Domain: ClockDomain> Clone for Clock<Domain> {
    fn clone(&self) -> Self {
        Self {
            state: std::sync::Arc::clone(&self.state),
            _domain: std::marker::PhantomData,
        }
    }
}


/// Future representing a clock tick
/// 
/// This is returned by `Clock::tick()` and should be awaited in async functions.
pub struct ClockTick<Domain: ClockDomain> {
    state: Arc<Mutex<ClockState>>,
    target_cycle: u64,
    _domain: PhantomData<Domain>,
}

impl<Domain: ClockDomain> std::future::Future for ClockTick<Domain> {
    type Output = ();
    
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut state = self.state.lock().unwrap();
        if state.cycle >= self.target_cycle {
            std::task::Poll::Ready(())
        } else {
            state.wakers.push(cx.waker().clone());
            std::task::Poll::Pending
        }
    }
}

/// Wrapper for sequential state in hardware modules
/// 
/// This type marks data that persists across clock cycles.
/// It separates "current" state from "next" state for proper hardware semantics.
pub struct State<T> {
    current: T,
    next: Option<T>,
}

impl<T> State<T> {
    /// Create new state with an initial value
    pub fn new(initial: T) -> Self {
        Self {
            current: initial,
            next: None,
        }
    }
    
    /// Read the current state value
    pub fn current(&self) -> &T {
        &self.current
    }
    
    /// Set the next state value (takes effect after clock edge)
    pub fn set_next(&mut self, value: T) {
        self.next = Some(value);
    }
    
    /// Advance state (for simulation - moves next to current)
    pub fn advance(&mut self) where T: Clone {
        if let Some(next) = self.next.take() {
            self.current = next;
        }
    }
    
    /// Check if next state has been set
    pub fn has_next(&self) -> bool {
        self.next.is_some()
    }
}

impl<T: Clone> State<T> {
    /// Get a clone of the current value
    pub fn current_clone(&self) -> T {
        self.current.clone()
    }
}

impl<T: Default> Default for State<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: fmt::Debug> fmt::Debug for State<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("current", &self.current)
            .field("next", &self.next)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bit_creation() {
        assert_eq!(Bit::ZERO.0, Logic::Zero);
        assert_eq!(Bit::ONE.0, Logic::One);
        assert_eq!(Bit::X.0, Logic::X);
    }
    
    #[test]
    fn test_bit_from_bool() {
        assert_eq!(Bit::from_bool(false), Bit::ZERO);
        assert_eq!(Bit::from_bool(true), Bit::ONE);
    }
    
    #[test]
    fn test_bit_logic_ops() {
        assert_eq!(!Bit::ZERO, Bit::ONE);
        assert_eq!(!Bit::ONE, Bit::ZERO);
        assert_eq!(Bit::ONE & Bit::ONE, Bit::ONE);
        assert_eq!(Bit::ONE & Bit::ZERO, Bit::ZERO);
        assert_eq!(Bit::ONE | Bit::ZERO, Bit::ONE);
        assert_eq!(Bit::ZERO | Bit::ZERO, Bit::ZERO);
        assert_eq!(Bit::ONE ^ Bit::ZERO, Bit::ONE);
        assert_eq!(Bit::ONE ^ Bit::ONE, Bit::ZERO);
    }
    
    #[test]
    fn test_bits_creation() {
        let zero: Bits<8> = Bits::zero();
        assert_eq!(zero.as_u128(), 0);
        
        let ones: Bits<8> = Bits::ones();
        assert_eq!(ones.as_u128(), 0xFF);
    }
    
    #[test]
    fn test_bits_from_u128() {
        let val: Bits<8> = Bits::from_u128(42);
        assert_eq!(val.as_u128(), 42);
        
        let val: Bits<16> = Bits::from_u128(0x1234);
        assert_eq!(val.as_u128(), 0x1234);
    }
    
    #[test]
    fn test_bits_add() {
        let a: Bits<8> = Bits::from_u128(10);
        let b: Bits<8> = Bits::from_u128(20);
        let sum = a + b;
        assert_eq!(sum.as_u128(), 30);
    }
    
    #[test]
    fn test_bits_shift() {
        let val: Bits<8> = Bits::from_u128(0b10110100);
        let left = val.shift_left(2);
        assert_eq!(left.as_u128(), 0b11010000);
        
        let right = val.shift_right(2);
        assert_eq!(right.as_u128(), 0b00101101);
    }
    
    #[test]
    fn test_state() {
        let mut state = State::new(42u32);
        assert_eq!(*state.current(), 42);
        
        state.set_next(100);
        assert!(state.has_next());
        
        state.advance();
        assert_eq!(*state.current(), 100);
        assert!(!state.has_next());
    }
    
    // Clock domain tests
    struct TestClkA;
    impl ClockDomain for TestClkA {}
    
    struct TestClkB;
    impl ClockDomain for TestClkB {}
    
    #[test]
    fn test_signal_same_domain() {
        let mut sig_a: Signal<TestClkA, Bits<8>> = Signal::new(Bits::from_u128(42));
        assert_eq!(sig_a.read().as_u128(), 42);
        
        sig_a.write(Bits::from_u128(100));
        assert_eq!(sig_a.read().as_u128(), 100);
    }
    
    #[test]
    fn test_clock_advance() {
        let mut clk: Clock<TestClkA> = Clock::new();
        assert_eq!(clk.cycle(), 0);
        
        clk.advance();
        assert_eq!(clk.cycle(), 1);
        
        clk.advance();
        assert_eq!(clk.cycle(), 2);
    }
}
