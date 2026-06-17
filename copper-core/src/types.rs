//! Core type system for Copper HDL
//! 
//! This module defines the foundational types for hardware design:
//! - `Logic`: Single Logic with 4-state logic
//! - `Bits<N>`: Logic vectors of compile-time width
//! - `Clock`: Clock source for synchronous logic

use std::marker::PhantomData;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::task::Waker;


// primitive logic values (0, 1, X)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Logic {
    Zero = 0,
    One = 1,
    X, // unknown
}

/// A single hardware logic value with 3-state logic (0, 1, X)
impl Logic {
    /// Create a new Logic from a boolean
    pub fn from_bool(b: bool) -> Self {
        if b { Self::One } else { Self::Zero }
    }
    
    /// Convert to boolean if possible (panics on X/Z)
    pub fn as_bool(&self) -> bool {
        match self {
            Logic::Zero => false,
            Logic::One => true,
            Logic::X => panic!("Cannot convert X to bool"),
        }
    }
    
    /// Check if this Logic is a valid boolean (not X or Z)
    pub fn is_valid(&self) -> bool {
        matches!(self, Logic::Zero | Logic::One)
    }
}

/// Convert a boolean to a Logic value
impl From<bool> for Logic {
    fn from(b: bool) -> Self {
        Self::from_bool(b)
    }
}

impl std::ops::Not for Logic {
    type Output = Logic;
    
    // should i have it throw an error for X and Z?
    fn not(self) -> Self::Output {
        match self {
            Logic::Zero => Logic::One,
            Logic::One => Logic::Zero,
            Logic::X => Logic::X,
        }
    }
}

impl std::ops::BitAnd for Logic {
    type Output = Logic;
    
    fn bitand(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Logic::Zero, _) | (_, Logic::Zero) => Logic::Zero,
            (Logic::One, Logic::One) => Logic::One,
            _ => Logic::X,
        }
    }
}

impl std::ops::BitAndAssign for Logic {
    fn bitand_assign(&mut self, rhs: Self) {
        *self = *self & rhs;
    }
}

impl std::ops::BitOr for Logic {
    type Output = Logic;
    
    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Logic::One, _) | (_, Logic::One) => Logic::One,
            (Logic::Zero, Logic::Zero) => Logic::Zero,
            _ => Logic::X,
        }
    }
}

impl std::ops::BitOrAssign for Logic {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}

impl std::ops::BitXor for Logic {
    type Output = Logic;
    
    fn bitxor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Logic::Zero, Logic::Zero) | (Logic::One, Logic::One) => Logic::Zero,
            (Logic::Zero, Logic::One) | (Logic::One, Logic::Zero) => Logic::One,
            _ => Logic::X,
        }
    }
}

impl std::ops::BitXorAssign for Logic {
    fn bitxor_assign(&mut self, rhs: Self) {
        *self = *self ^ rhs;
    }
}

impl fmt::Display for Logic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Logic::Zero => write!(f, "0"),
            Logic::One => write!(f, "1"),
            Logic::X => write!(f, "X"),
        }
    }
}

impl fmt::Binary for Logic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Logic::Zero => write!(f, "0"),
            Logic::One => write!(f, "1"),
            Logic::X => write!(f, "X"),
        }
    }
}

impl fmt::LowerHex for Logic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Binary::fmt(self, f)
    }
}

impl fmt::UpperHex for Logic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Logic::Zero => write!(f, "0"),
            Logic::One => write!(f, "1"),
            Logic::X => write!(f, "X"),
        }
    }
}

impl fmt::Octal for Logic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Binary::fmt(self, f)
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
    /// TODO: What if there isn't enough bits to represent the value?
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

    // TODO: Add other conversion methods (e.g., as_i128, as_f64, etc.)
    
    /// Get the Logic at index i (LSB = 0)
    pub fn get(&self, i: usize) -> Logic {
        assert!(i < N, "Logic index out of bounds");
        self.bits[i]
    }
    
    /// Set the Logic at index i
    pub fn set(&mut self, i: usize, logic: Logic) {
        assert!(i < N, "Bits index out of bounds");
        self.bits[i] = logic;
    }
    
    /// Get the internal array
    pub fn as_array(&self) -> &[Logic; N] {
        &self.bits
    }
    
    /// Get mutable internal array
    pub fn as_array_mut(&mut self) -> &mut [Logic; N] {
        &mut self.bits
    }
    
    /// Check if all bits are valid (not X or Z)
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
    /// TODO: Check for correctness
    pub fn shift_right(&self, n: usize) -> Self {
        let mut result = [Logic::Zero; N];
        for i in 0..(N.saturating_sub(n)) {
            result[i] = self.bits[i + n];
        }
        Self { bits: result }
    }
    
    /// Set the LSB (Logic 0) to a new value
    pub fn with_lsb(&self, logic: Logic) -> Self {
        let mut result = self.clone();
        result.bits[0] = logic;
        result
    }
    
    /// Set the MSB (Logic N-1) to a new value
    pub fn with_msb(&self, logic: Logic) -> Self {
        let mut result = self.clone();
        result.bits[N - 1] = logic;
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
                Logic::X => write!(f, "x")?,
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
                Logic::X => write!(f, "x")?,
            }
        }
        Ok(())
    }
}

impl<const N: usize> fmt::Binary for Bits<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for bit in self.bits.iter().rev() {
            match bit {
                Logic::Zero => write!(f, "0")?,
                Logic::One => write!(f, "1")?,
                Logic::X => write!(f, "x")?,
            }
        }
        Ok(())
    }
}

impl<const N: usize> fmt::LowerHex for Bits<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let num_digits = (N + 3) / 4;
        for d in (0..num_digits).rev() {
            let lo = d * 4;
            let hi = (d * 4 + 3).min(N - 1);
            if (lo..=hi).any(|i| self.bits[i] == Logic::X) {
                write!(f, "x")?;
            } else {
                let mut val = 0u8;
                for i in lo..=hi {
                    if self.bits[i] == Logic::One {
                        val |= 1 << (i - lo);
                    }
                }
                write!(f, "{:x}", val)?;
            }
        }
        Ok(())
    }
}

impl<const N: usize> fmt::UpperHex for Bits<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let num_digits = (N + 3) / 4;
        for d in (0..num_digits).rev() {
            let lo = d * 4;
            let hi = (d * 4 + 3).min(N - 1);
            if (lo..=hi).any(|i| self.bits[i] == Logic::X) {
                write!(f, "X")?;
            } else {
                let mut val = 0u8;
                for i in lo..=hi {
                    if self.bits[i] == Logic::One {
                        val |= 1 << (i - lo);
                    }
                }
                write!(f, "{:X}", val)?;
            }
        }
        Ok(())
    }
}

impl<const N: usize> fmt::Octal for Bits<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let num_digits = (N + 2) / 3;
        for d in (0..num_digits).rev() {
            let lo = d * 3;
            let hi = (d * 3 + 2).min(N - 1);
            if (lo..=hi).any(|i| self.bits[i] == Logic::X) {
                write!(f, "x")?;
            } else {
                let mut val = 0u8;
                for i in lo..=hi {
                    if self.bits[i] == Logic::One {
                        val |= 1 << (i - lo);
                    }
                }
                write!(f, "{:o}", val)?;
            }
        }
        Ok(())
    }
}

// Arithmetic operations

// TODO:
// Maybe don't support these?
// You would need to add in a adder circuit for it actually do do something.
// impl<const N: usize> std::ops::Add for Bits<N> {
//     type Output = Self;
    
//     fn add(self, rhs: Self) -> Self::Output {
//         let a = self.as_u128();
//         let b = rhs.as_u128();
//         let sum = a.wrapping_add(b);
//         Self::from_u128(sum)
//     }
// }

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

// TODO: Add other bitwise operations (AND, OR, XOR)

/// Trait for types that have a defined unknown/X state.
///
/// Implemented by all built-in logic types (`Logic`, `Bit`, `Bits<N>`) and
/// their tuples.  The executor uses this when a combinational loop is detected:
/// rather than panicking, it sets the oscillating signal to `unknown()` so
/// that X propagates through downstream combinational logic and the simulation
/// reaches a fixed point — matching real Verilog simulator behaviour.
pub trait HasUnknown {
    fn unknown() -> Self;
}

impl HasUnknown for Logic {
    fn unknown() -> Self { Logic::X }
}

impl<const N: usize> HasUnknown for Bits<N> {
    fn unknown() -> Self { Bits::x() }
}

impl<A: HasUnknown, B: HasUnknown> HasUnknown for (A, B) {
    fn unknown() -> Self { (A::unknown(), B::unknown()) }
}

impl<A: HasUnknown, B: HasUnknown, C: HasUnknown> HasUnknown for (A, B, C) {
    fn unknown() -> Self { (A::unknown(), B::unknown(), C::unknown()) }
}

impl<A: HasUnknown, B: HasUnknown, C: HasUnknown, D: HasUnknown> HasUnknown for (A, B, C, D) {
    fn unknown() -> Self { (A::unknown(), B::unknown(), C::unknown(), D::unknown()) }
}

/// Traits for types that listen to clock edges (synchronous logic)
pub(crate) trait ClockEdgeListener: Send + Sync {
    fn on_posedge(&self);
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

#[derive(Debug)]
struct ClockState {
    cycle: u64,
    wakers: Vec<Waker>,
    listeners: Vec<std::sync::Weak<dyn ClockEdgeListener>>,
}

/// Clock source for synchronous logic
/// 
/// Represents a clock signal that can be awaited in async state machines.
/// Each clock has an associated domain type for safety.
#[derive(Debug)]
pub struct Clock<Domain: ClockDomain> {
    state: Arc<Mutex<ClockState>>, // shared state for tracking clock cycles and waiting tasks
    _domain: PhantomData<Domain>, // phantom type to associate with clock domain
}

impl<Domain: ClockDomain> Clock<Domain> {
    /// Create a new clock starting at cycle 0
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ClockState {
                cycle: 0, // starts at t=0
                wakers: Vec::new(), // no wakers initially
                listeners: Vec::new(), // no listeners initially
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

        // call on_posedge for all listeners
        state.listeners.retain(|weak_listener| {
            match weak_listener.upgrade() {
                Some(listener) => {
                    listener.on_posedge();
                    true // keep in list
                },
                None => false, // remove if listener was dropped
            }
        });
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
        // TODO: add an error if overflow occurs (unlikely in practice)
        let target = self.cycle().wrapping_add(1); 
        ClockTick {
            state: Arc::clone(&self.state), // get same state
            target_cycle: target, // target is next cycle
            _domain: PhantomData,
        }
    }

    pub(crate) fn register_listener(&self, listener: std::sync::Weak<dyn ClockEdgeListener>) {
        let mut state = self.state.lock().unwrap();
        state.listeners.push(listener);
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


#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bit_creation() {
        assert_eq!(Logic::Zero, Logic::Zero);
        assert_eq!(Logic::One, Logic::One);
        assert_eq!(Logic::X, Logic::X);
    }
    
    #[test]
    fn test_bit_from_bool() {
        assert_eq!(Logic::from_bool(false), Logic::Zero);
        assert_eq!(Logic::from_bool(true), Logic::One);
    }
    
    #[test]
    fn test_bit_logic_ops() {
        assert_eq!(!Logic::Zero, Logic::One);
        assert_eq!(!Logic::One, Logic::Zero);
        assert_eq!(Logic::One & Logic::One, Logic::One);
        assert_eq!(Logic::One & Logic::Zero, Logic::Zero);
        assert_eq!(Logic::One | Logic::Zero, Logic::One);
        assert_eq!(Logic::Zero | Logic::Zero, Logic::Zero);
        assert_eq!(Logic::One ^ Logic::Zero, Logic::One);
        assert_eq!(Logic::One ^ Logic::One, Logic::Zero);
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
        assert_eq!(a.as_u128() + b.as_u128(), 30);
    }
    
    #[test]
    fn test_bits_shift() {
        let val: Bits<8> = Bits::from_u128(0b10110100);
        let left = val.shift_left(2);
        assert_eq!(left.as_u128(), 0b11010000);
        
        let right = val.shift_right(2);
        assert_eq!(right.as_u128(), 0b00101101);
    }
    
    // Tests to add: TODO!
    // logic: from bool and as bool
}
