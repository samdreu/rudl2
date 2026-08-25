//! Core type system for Copper HDL
//! 
//! This module defines the foundational types for hardware design:
//! - `Logic`: Single logic value with 3-state logic (0, 1, X)
//! - `Bits<N>`: Logic vectors of compile-time width
//! - `Clock`: Clock source for synchronous logic

use std::marker::PhantomData;
use std::fmt;
use std::sync::{Arc, Mutex};


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
    
    /// Convert to boolean if possible (panics on X)
    pub fn as_bool(&self) -> bool {
        match self {
            Logic::Zero => false,
            Logic::One => true,
            Logic::X => panic!("Cannot convert X to bool"),
        }
    }
    
    /// Check if this Logic is a valid boolean (not X)
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

/// Convert Logic to a boolean if possible (panics on X)
impl Into<bool> for Logic {
    fn into(self) -> bool {
        self.as_bool()
    }
}

impl std::ops::Not for Logic {
    type Output = Logic;
    
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

/// A bit vector of compile-time constant width N
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bits<const N: usize> {
    bits: [Logic; N],
}

impl<const N: usize> Bits<N> {
    /// A bit vector of all zeros — which is also the *value* zero.
    ///
    /// Its counterpart is [`Bits::all_ones`], deliberately not named `one`:
    /// all-ones is `2^N - 1`, not 1. For the value 1, use
    /// `Bits::from_lit::<1>()`.
    pub fn zero() -> Self {
        Self { bits: [Logic::Zero; N] }
    }

    /// A bit vector with every bit set — the value `2^N - 1`, **not** 1.
    ///
    /// `Bits::<3>::all_ones()` is 7. This was called `one()`, which read as the
    /// value 1 next to [`Bits::zero`] (where the all-zeros/value-zero reading
    /// happens to coincide) and silently produced an all-ones mask instead — it
    /// cost a BaseJump counter 49 wrong cycles before the reference caught it.
    ///
    /// For the value 1, use `Bits::from_lit::<1>()`.
    ///
    /// ```
    /// use copper_core::Bits;
    /// assert_eq!(Bits::<3>::all_ones().as_u128(), 7);
    /// assert_eq!(Bits::<3>::from_lit::<1>().as_u128(), 1);
    /// ```
    pub fn all_ones() -> Self {
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
    
    pub fn from_u8(val: u8) -> Self {
        const { assert!(N >= 8, "Bits<N> too small for u8; N must be >= 8") };
        Self::from_uint(val as u128)
    }

    pub fn from_u16(val: u16) -> Self {
        const { assert!(N >= 16, "Bits<N> too small for u16; N must be >= 16") };
        Self::from_uint(val as u128)
    }

    pub fn from_u32(val: u32) -> Self {
        const { assert!(N >= 32, "Bits<N> too small for u32; N must be >= 32") };
        Self::from_uint(val as u128)
    }

    pub fn from_u64(val: u64) -> Self {
        const { assert!(N >= 64, "Bits<N> too small for u64; N must be >= 64") };
        Self::from_uint(val as u128)
    }

    pub fn from_u128(val: u128) -> Self {
        const { assert!(N >= 128, "Bits<N> too small for u128; N must be >= 128") };
        Self::from_uint(val)
    }

    /// Convert a runtime `usize` to `Bits<N>` — equivalent to SV's `N'(expr)` cast.
    /// Panics if `val` doesn't fit in `N` bits.
    pub fn from_usize(val: usize) -> Self {
        let fits = N >= 64 || val < (1usize << N);
        assert!(fits, "value {val} does not fit in Bits<{N}>");
        Self::from_uint(val as u128)
    }

    /// Create `Bits<N>` from a compile-time constant.
    /// The required bit width is inferred from the constant value itself,
    /// and the call is rejected at compile time if `N` is too narrow.
    ///
    /// ```compile_fail
    /// let _: Bits<4> = Bits::from_lit::<31>(); // 31 needs 5 bits — compile error
    /// ```
    pub fn from_lit<const VAL: u128>() -> Self {
        const { assert!(N >= 128 - VAL.leading_zeros() as usize,
            "constant value does not fit in Bits<N>: N is too narrow for this literal") };
        Self::from_uint(VAL)
    }

    fn from_uint(val: u128) -> Self {
        let mut bits = [Logic::Zero; N];
        for i in 0..N.min(128) {
            bits[i] = if (val >> i) & 1 == 1 { Logic::One } else { Logic::Zero };
        }
        Self { bits }
    }
    
    pub fn as_u8(&self) -> u8 {
        const { assert!(N <= 8, "Bits<N> too wide for u8; N must be <= 8") };
        self.as_uint() as u8
    }

    pub fn as_u16(&self) -> u16 {
        const { assert!(N <= 16, "Bits<N> too wide for u16; N must be <= 16") };
        self.as_uint() as u16
    }

    pub fn as_u32(&self) -> u32 {
        const { assert!(N <= 32, "Bits<N> too wide for u32; N must be <= 32") };
        self.as_uint() as u32
    }

    pub fn as_u64(&self) -> u64 {
        const { assert!(N <= 64, "Bits<N> too wide for u64; N must be <= 64") };
        self.as_uint() as u64
    }

    pub fn as_u128(&self) -> u128 {
        const { assert!(N <= 128, "Bits<N> too wide for u128; N must be <= 128") };
        self.as_uint()
    }

    pub fn as_usize(&self) -> usize {
        const { assert!(N <= 64, "Bits<N> too wide for usize; N must be <= 64") };
        self.as_uint() as usize
    }

    fn as_uint(&self) -> u128 {
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

    // ── Reduction operators ───────────────────────────────────────────────────

    /// AND-reduce: One iff all bits are One, Zero if any is Zero, X otherwise.
    pub fn and_reduce(&self) -> Logic {
        self.bits.iter().fold(Logic::One, |acc, &b| acc & b)
    }

    /// OR-reduce: One if any bit is One, Zero if all are Zero, X otherwise.
    pub fn or_reduce(&self) -> Logic {
        self.bits.iter().fold(Logic::Zero, |acc, &b| acc | b)
    }

    /// XOR-reduce: parity of all bits, X if any bit is X.
    pub fn xor_reduce(&self) -> Logic {
        self.bits.iter().fold(Logic::Zero, |acc, &b| acc ^ b)
    }

    pub fn nand_reduce(&self) -> Logic { !self.and_reduce() }
    pub fn nor_reduce(&self)  -> Logic { !self.or_reduce()  }
    pub fn xnor_reduce(&self) -> Logic { !self.xor_reduce() }

    // ── Arithmetic shift right ────────────────────────────────────────────────

    /// Shift right, filling vacated MSBs with the sign bit (bits[N-1]).
    pub fn arithmetic_shift_right(&self, n: usize) -> Self {
        let sign = self.bits[N - 1];
        let mut result = [Logic::Zero; N];
        let keep = N.saturating_sub(n);
        for i in 0..keep { result[i] = self.bits[i + n]; }
        for i in keep..N { result[i] = sign; }
        Self { bits: result }
    }

    // ── X-aware comparisons (Verilog `==` semantics) ──────────────────────────

    /// Returns X if either operand contains any X bit, otherwise One/Zero.
    pub fn eq_logic(&self, other: &Self) -> Logic {
        if !self.is_valid() || !other.is_valid() { return Logic::X; }
        Logic::from_bool(self == other)
    }

    pub fn ne_logic(&self, other: &Self) -> Logic { !self.eq_logic(other) }

    /// Unsigned less-than returning Logic. Returns X if either operand has X bits.
    pub fn lt_logic(&self, other: &Self) -> Logic {
        if !self.is_valid() || !other.is_valid() { return Logic::X; }
        Logic::from_bool(self.as_u128() < other.as_u128())
    }

    pub fn le_logic(&self, other: &Self) -> Logic {
        if !self.is_valid() || !other.is_valid() { return Logic::X; }
        Logic::from_bool(self.as_u128() <= other.as_u128())
    }

    pub fn gt_logic(&self, other: &Self) -> Logic {
        if !self.is_valid() || !other.is_valid() { return Logic::X; }
        Logic::from_bool(self.as_u128() > other.as_u128())
    }

    pub fn ge_logic(&self, other: &Self) -> Logic {
        if !self.is_valid() || !other.is_valid() { return Logic::X; }
        Logic::from_bool(self.as_u128() >= other.as_u128())
    }

    // ── Width conversion ──────────────────────────────────────────────────────

    /// Zero-extend to M bits. Panics if M < N.
    pub fn zero_extend<const M: usize>(&self) -> Bits<M> {
        assert!(M >= N, "zero_extend: target width {M} < source width {N}");
        let mut bits = [Logic::Zero; M];
        bits[..N].copy_from_slice(&self.bits);
        Bits { bits }
    }

    /// Sign-extend to M bits using bits[N-1] as the sign bit. Panics if M < N.
    pub fn sign_extend<const M: usize>(&self) -> Bits<M> {
        assert!(M >= N, "sign_extend: target width {M} < source width {N}");
        let sign = self.bits[N - 1];
        let mut bits = [Logic::Zero; M];
        bits[..N].copy_from_slice(&self.bits);
        for b in &mut bits[N..] { *b = sign; }
        Bits { bits }
    }

    /// Keep the M least-significant bits. Panics if M > N.
    pub fn truncate<const M: usize>(&self) -> Bits<M> {
        assert!(M <= N, "truncate: target width {M} > source width {N}");
        let mut bits = [Logic::Zero; M];
        bits.copy_from_slice(&self.bits[..M]);
        Bits { bits }
    }

    // ── Bit selection and concatenation ───────────────────────────────────────

    /// Extract LEN bits starting at bit index `lo`. Verilog: `self[lo +: LEN]`.
    pub fn part_select<const LEN: usize>(&self, lo: usize) -> Bits<LEN> {
        assert!(
            lo + LEN <= N,
            "part_select: [{lo}..+{LEN}) out of bounds for width {N}"
        );
        let mut bits = [Logic::Zero; LEN];
        bits.copy_from_slice(&self.bits[lo..lo + LEN]);
        Bits { bits }
    }

    /// Concatenate: self in the MSBs, other in the LSBs. OUT must equal N + M.
    /// Verilog: `{self, other}`.
    pub fn concat<const M: usize, const OUT: usize>(&self, other: &Bits<M>) -> Bits<OUT> {
        assert_eq!(OUT, N + M, "concat: OUT ({OUT}) must equal N ({N}) + M ({M})");
        let mut bits = [Logic::Zero; OUT];
        bits[..M].copy_from_slice(&other.bits);
        bits[M..].copy_from_slice(&self.bits);
        Bits { bits }
    }

    /// Replicate self to fill OUT bits. OUT must be a multiple of N.
    /// Verilog: `{(OUT/N){self}}`.
    pub fn replicate<const OUT: usize>(&self) -> Bits<OUT> {
        assert_eq!(OUT % N, 0, "replicate: OUT ({OUT}) must be a multiple of N ({N})");
        let mut bits = [Logic::Zero; OUT];
        for chunk in bits.chunks_exact_mut(N) {
            chunk.copy_from_slice(&self.bits);
        }
        Bits { bits }
    }

    // ── Multiplexer ───────────────────────────────────────────────────────────

    /// Select a or b based on sel. Verilog: `sel ? a : b`.
    /// When sel is X, bits that agree between a and b pass through; others become X.
    pub fn mux(sel: Logic, a: &Self, b: &Self) -> Self {
        match sel {
            Logic::One  => a.clone(),
            Logic::Zero => b.clone(),
            Logic::X => {
                let mut bits = [Logic::X; N];
                for i in 0..N {
                    if a.bits[i] == b.bits[i] { bits[i] = a.bits[i]; }
                }
                Self { bits }
            }
        }
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

impl<const N: usize> fmt::Binary for Bits<N> {
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

// ── Bitwise operators for Bits<N> ────────────────────────────────────────────

impl<const N: usize> std::ops::Not for Bits<N> {
    type Output = Self;
    fn not(self) -> Self::Output {
        let mut bits = [Logic::Zero; N];
        for i in 0..N { bits[i] = !self.bits[i]; }
        Self { bits }
    }
}

impl<const N: usize> std::ops::BitAnd for Bits<N> {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        let mut bits = [Logic::Zero; N];
        for i in 0..N { bits[i] = self.bits[i] & rhs.bits[i]; }
        Self { bits }
    }
}

impl<const N: usize> std::ops::BitAndAssign for Bits<N> {
    fn bitand_assign(&mut self, rhs: Self) { *self = self.clone() & rhs; }
}

impl<const N: usize> std::ops::BitOr for Bits<N> {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        let mut bits = [Logic::Zero; N];
        for i in 0..N { bits[i] = self.bits[i] | rhs.bits[i]; }
        Self { bits }
    }
}

impl<const N: usize> std::ops::BitOrAssign for Bits<N> {
    fn bitor_assign(&mut self, rhs: Self) { *self = self.clone() | rhs; }
}

impl<const N: usize> std::ops::BitXor for Bits<N> {
    type Output = Self;
    fn bitxor(self, rhs: Self) -> Self::Output {
        let mut bits = [Logic::Zero; N];
        for i in 0..N { bits[i] = self.bits[i] ^ rhs.bits[i]; }
        Self { bits }
    }
}

impl<const N: usize> std::ops::BitXorAssign for Bits<N> {
    fn bitxor_assign(&mut self, rhs: Self) { *self = self.clone() ^ rhs; }
}

// ── Arithmetic operators (wrapping, valid for N ≤ 128) ───────────────────────

impl<const N: usize> std::ops::Add for Bits<N> {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        if !self.is_valid() || !rhs.is_valid() { return Self::x(); }
        let mask = if N < 128 { (1u128 << N) - 1 } else { u128::MAX };
        Self::from_uint(self.as_u128().wrapping_add(rhs.as_u128()) & mask)
    }
}

impl<const N: usize> std::ops::Add for &Bits<N> {
    type Output = Bits<N>;
    fn add(self, rhs: Self) -> Self::Output {
        if !self.is_valid() || !rhs.is_valid() { return Bits::<N>::x(); }
        let mask = if N < 128 { (1u128 << N) - 1 } else { u128::MAX };
        Bits::<N>::from_uint(self.as_u128().wrapping_add(rhs.as_u128()) & mask)
    }
}

impl<const N: usize> std::ops::Sub for Bits<N> {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        if !self.is_valid() || !rhs.is_valid() { return Self::x(); }
        let mask = if N < 128 { (1u128 << N) - 1 } else { u128::MAX };
        Self::from_uint(self.as_u128().wrapping_sub(rhs.as_u128()) & mask)
    }
}

impl<const N: usize> std::ops::Sub for &Bits<N> {
    type Output = Bits<N>;
    fn sub(self, rhs: Self) -> Self::Output {
        if !self.is_valid() || !rhs.is_valid() { return Bits::<N>::x(); }
        let mask = if N < 128 { (1u128 << N) - 1 } else { u128::MAX };
        Bits::<N>::from_uint(self.as_u128().wrapping_sub(rhs.as_u128()) & mask)
    }
}

impl<const N: usize> std::ops::Mul for Bits<N> {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        if !self.is_valid() || !rhs.is_valid() { return Self::x(); }
        let mask = if N < 128 { (1u128 << N) - 1 } else { u128::MAX };
        Self::from_uint(self.as_u128().wrapping_mul(rhs.as_u128()) & mask)
    }
}

impl<const N: usize> std::ops::Div for Bits<N> {
    type Output = Self;
    fn div(self, rhs: Self) -> Self::Output {
        if !self.is_valid() || !rhs.is_valid() { return Self::x(); }
        if rhs.as_u128() == 0 { return Self::x(); }
        Self::from_uint(self.as_u128() / rhs.as_u128())
    }
}

impl<const N: usize> std::ops::Rem for Bits<N> {
    type Output = Self;
    fn rem(self, rhs: Self) -> Self::Output {
        if !self.is_valid() || !rhs.is_valid() { return Self::x(); }
        if rhs.as_u128() == 0 { return Self::x(); }
        Self::from_uint(self.as_u128() % rhs.as_u128())
    }
}

impl<const N: usize> std::ops::Neg for Bits<N> {
    type Output = Self;
    fn neg(self) -> Self::Output {
        if !self.is_valid() { return Self::x(); }
        let mask = if N < 128 { (1u128 << N) - 1 } else { u128::MAX };
        Self::from_uint(self.as_u128().wrapping_neg() & mask)
    }
}

// ── Shift operator traits (logical shifts) ────────────────────────────────────

impl<const N: usize> std::ops::Shl<usize> for Bits<N> {
    type Output = Self;
    fn shl(self, rhs: usize) -> Self::Output { self.shift_left(rhs) }
}

impl<const N: usize> std::ops::Shr<usize> for Bits<N> {
    type Output = Self;
    fn shr(self, rhs: usize) -> Self::Output { self.shift_right(rhs) }
}

// ── Slice access ─────────────────────────────────────────────────────────────

impl<const N: usize> std::ops::Deref for Bits<N> {
    type Target = [Logic];
    fn deref(&self) -> &[Logic] { &self.bits }
}

impl<const N: usize> std::ops::DerefMut for Bits<N> {
    fn deref_mut(&mut self) -> &mut [Logic] { &mut self.bits }
}

/// Trait for types that have a defined unknown/X state.
///
/// Implemented by all built-in logic types (`Logic`, `Bits<N>`) and
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

// ── Conversions ───────────────────────────────────────────────────────────────

impl From<Logic> for Bits<1> {
    fn from(l: Logic) -> Self { Self { bits: [l] } }
}

impl From<Bits<1>> for Logic {
    fn from(b: Bits<1>) -> Self { b.bits[0] }
}

impl<const N: usize> From<u128> for Bits<N> {
    fn from(val: u128) -> Self { Self::from_u128(val) }
}

impl<const N: usize> From<[Logic; N]> for Bits<N> {
    fn from(bits: [Logic; N]) -> Self { Self::from_array(bits) }
}

impl<const N: usize> TryFrom<&[Logic]> for Bits<N> {
    type Error = String;
    fn try_from(slice: &[Logic]) -> Result<Self, Self::Error> {
        if slice.len() != N {
            return Err(format!("slice length {} does not match Bits<{}>", slice.len(), N));
        }
        Ok(Self::from_slice(slice))
    }
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

thread_local! {
    /// Whether a `clk.tick()` resolves in the current settle pass, keyed per clock
    /// domain (by `TypeId`) so that ticking one domain's clock cannot perturb
    /// another domain's phase-gated futures. The executor enables this in the
    /// POST-edge pass only, so a reaction's post-tick code runs after
    /// `clk.advance()` within the same `tick_clock` (the post-edge continuation
    /// convention — a register clocked at edge N is observable in cycle N). Each loop
    /// reaction still advances by exactly one tick per `tick_clock` — never compressed
    /// into the same call as the previous reaction. See
    /// design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md. A domain with no entry yet
    /// defaults to `true` so bare-future unit tests that don't drive the phase
    /// still progress.
    static TICK_RESOLVING: std::cell::RefCell<std::collections::HashMap<std::any::TypeId, bool>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Executor hook: mark whether `Domain`'s `clk.tick()` resolves in the current
/// settle pass. Scoped per clock domain — setting this for one domain has no
/// effect on any other domain's tasks.
pub fn set_tick_resolving<Domain: ClockDomain>(resolves: bool) {
    TICK_RESOLVING.with(|c| {
        c.borrow_mut().insert(std::any::TypeId::of::<Domain>(), resolves);
    });
}

fn tick_resolves_now<Domain: ClockDomain>() -> bool {
    TICK_RESOLVING.with(|c| {
        c.borrow().get(&std::any::TypeId::of::<Domain>()).copied().unwrap_or(true)
    })
}

impl<Domain: ClockDomain> std::future::Future for ClockTick<Domain> {
    type Output = ();

    // No waker registration: the simulation executor doesn't use one (it polls
    // every task unconditionally every delta cycle via a noop waker — see
    // `HardwareExecutor::poll_tasks`), and this future previously pushed a new
    // `Waker` clone into `state.wakers` on every Pending poll with no dedup,
    // growing unboundedly across the delta cycles within a single settle phase
    // before `advance()` finally drained it. Removed rather than fixed to dedup:
    // a real waker-based executor is not on the roadmap (the fixed-point
    // delta-cycle model has no equivalent in `futures`' wake-driven primitives),
    // so there was nothing for the registration to ever usefully serve.
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let state = self.state.lock().unwrap();
        if state.cycle >= self.target_cycle && tick_resolves_now::<Domain>() {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logic_from_bool() {
        assert_eq!(Logic::from_bool(false), Logic::Zero);
        assert_eq!(Logic::from_bool(true), Logic::One);
    }

    #[test]
    fn test_logic_as_bool() {
        assert_eq!(Logic::Zero.as_bool(), false);
        assert_eq!(Logic::One.as_bool(), true);
    }

    #[test]
    #[should_panic(expected = "Cannot convert X to bool")]
    fn test_logic_as_bool_panic() {
        Logic::X.as_bool();
    }

    #[test]
    fn test_valid_logic() {
        assert!(Logic::Zero.is_valid());
        assert!(Logic::One.is_valid());
    }

    #[test]
    fn test_bool_to_logic() {
        assert_eq!(Logic::from(false), Logic::Zero);
        assert_eq!(Logic::from(true), Logic::One);
    }

    #[test]
    fn test_logic_into_bool() {
        let zero: bool = Logic::Zero.into();
        let one: bool = Logic::One.into();
        assert_eq!(zero, false);
        assert_eq!(one, true);
    }

    #[test]
    #[should_panic(expected = "Cannot convert X to bool")]
    fn test_logic_into_bool_panic() {
        let _x: bool = Logic::X.into();
    }

    #[test]
    fn test_not() {
        assert_eq!(!Logic::Zero, Logic::One);
        assert_eq!(!Logic::One, Logic::Zero);
        assert_eq!(!Logic::X, Logic::X);
    }

    #[test]
    fn test_and() {
        assert_eq!(Logic::Zero & Logic::Zero, Logic::Zero);
        assert_eq!(Logic::Zero & Logic::One, Logic::Zero);
        assert_eq!(Logic::One & Logic::Zero, Logic::Zero);
        assert_eq!(Logic::One & Logic::One, Logic::One);
        assert_eq!(Logic::X & Logic::Zero, Logic::Zero);
        assert_eq!(Logic::X & Logic::One, Logic::X);
        assert_eq!(Logic::X & Logic::X, Logic::X);
    }

    #[test]
    fn test_bit_and_assign() {
        let mut a = Logic::One;
        a &= Logic::Zero;
        assert_eq!(a, Logic::Zero);

        let mut b = Logic::X;
        b &= Logic::One;
        assert_eq!(b, Logic::X);

        let mut c = Logic::X;
        c &= Logic::X;
        assert_eq!(c, Logic::X);

        let mut d = Logic::Zero;
        d &= Logic::X;
        assert_eq!(d, Logic::Zero);

        let mut e = Logic::One;
        e &= Logic::X;
        assert_eq!(e, Logic::X);

        let mut f = Logic::X;
        f &= Logic::Zero;
        assert_eq!(f, Logic::Zero);

        let mut g = Logic::X;
        g &= Logic::X;
        assert_eq!(g, Logic::X);
    }

    #[test]
    fn test_or() {
        assert_eq!(Logic::Zero | Logic::Zero, Logic::Zero);
        assert_eq!(Logic::Zero | Logic::One, Logic::One);
        assert_eq!(Logic::One | Logic::Zero, Logic::One);
        assert_eq!(Logic::One | Logic::One, Logic::One);
        assert_eq!(Logic::X | Logic::Zero, Logic::X);
        assert_eq!(Logic::X | Logic::One, Logic::One);
        assert_eq!(Logic::X | Logic::X, Logic::X);
    }

    #[test]
    fn test_bit_or_assign() {
        let mut a = Logic::Zero;
        a |= Logic::Zero;
        assert_eq!(a, Logic::Zero);

        let mut b = Logic::Zero;
        b |= Logic::One;
        assert_eq!(b, Logic::One);

        let mut c = Logic::X;
        c |= Logic::Zero;
        assert_eq!(c, Logic::X);

        let mut d = Logic::X;
        d |= Logic::One;
        assert_eq!(d, Logic::One);

        let mut e = Logic::X;
        e |= Logic::X;
        assert_eq!(e, Logic::X);
    }

    #[test]
    fn test_xor() {
        assert_eq!(Logic::Zero ^ Logic::Zero, Logic::Zero);
        assert_eq!(Logic::Zero ^ Logic::One, Logic::One);
        assert_eq!(Logic::One ^ Logic::Zero, Logic::One);
        assert_eq!(Logic::One ^ Logic::One, Logic::Zero);
        assert_eq!(Logic::X ^ Logic::Zero, Logic::X);
        assert_eq!(Logic::X ^ Logic::One, Logic::X);
        assert_eq!(Logic::X ^ Logic::X, Logic::X);
    }

    #[test]
    fn test_bit_xor_assign() {
        let mut a = Logic::Zero;
        a ^= Logic::Zero;
        assert_eq!(a, Logic::Zero);

        let mut b = Logic::Zero;
        b ^= Logic::One;
        assert_eq!(b, Logic::One);

        let mut c = Logic::X;
        c ^= Logic::Zero;
        assert_eq!(c, Logic::X);

        let mut d = Logic::X;
        d ^= Logic::One;
        assert_eq!(d, Logic::X);

        let mut e = Logic::X;
        e ^= Logic::X;
        assert_eq!(e, Logic::X);
    }

    #[test]
    fn test_logic_display() {
        assert_eq!(format!("{}", Logic::Zero), "0");
        assert_eq!(format!("{}", Logic::One), "1");
        assert_eq!(format!("{}", Logic::X), "X");
    }

    #[test]
    fn test_logic_debug() {
        assert_eq!(format!("{:?}", Logic::Zero), "Zero");
        assert_eq!(format!("{:?}", Logic::One), "One");
        assert_eq!(format!("{:?}", Logic::X), "X");
    }

    #[test]
    fn test_zero_bits() {
        let bits: Bits<8> = Bits::zero();

        assert_eq!(bits.as_u128(), 0);
        assert_eq!(format!("{}", bits), "00000000");
        assert!(bits.as_array().iter().all(|bit| *bit == Logic::Zero));
    }

    #[test]
    fn test_all_ones_bits() {
        let bits: Bits<8> = Bits::all_ones();

        assert_eq!(bits.as_u128(), 255);
        assert_eq!(format!("{}", bits), "11111111");
        assert!(bits.as_array().iter().all(|bit| *bit == Logic::One));
    }

    #[test]
    fn test_x_bits() {
        let bits: Bits<8> = Bits::x();

        assert_eq!(format!("{}", bits), "XXXXXXXX");
        assert!(bits.as_array().iter().all(|bit| *bit == Logic::X));
    }

    #[test]
    fn test_from_array() {
        // [One, Zero, X, One] stores bits[0]=One(LSB)..bits[3]=One(MSB).
        // Display is MSB-first: bits[3]=One, bits[2]=X, bits[1]=Zero, bits[0]=One → "1X01"
        let bits = Bits::from_array([Logic::One, Logic::Zero, Logic::X, Logic::One]);

        assert_eq!(format!("{}", bits), "1X01");
    }

    #[test]
    fn test_from_slice() {
        let bits: Bits<4> = Bits::from_slice(&[Logic::One, Logic::Zero, Logic::X, Logic::One]);

        assert_eq!(format!("{}", bits), "1X01");
    }

    #[test]
    fn test_from_u128() {
        let bits: Bits<8> = Bits::from_lit::<255>();

        assert_eq!(format!("{}", bits), "11111111");

    }

    #[test]
    fn test_bits_default_matches_zero() {
        let bits: Bits<6> = Default::default();

        assert_eq!(bits, Bits::zero());
    }

    #[test]
    fn test_bits_get_set_and_array_access() {
        let mut bits: Bits<4> = Bits::zero();

        assert_eq!(bits.get(0), Logic::Zero);
        assert_eq!(bits.get(3), Logic::Zero);

        bits.set(1, Logic::One);
        bits.set(3, Logic::X);

        assert_eq!(bits.get(1), Logic::One);
        assert_eq!(bits.get(3), Logic::X);
        assert_eq!(bits.as_array(), &[Logic::Zero, Logic::One, Logic::Zero, Logic::X]);

        let array = bits.as_array_mut();
        array[0] = Logic::One;
        array[3] = Logic::Zero; // clear X so as_u128 doesn't panic

        assert_eq!(bits.get(0), Logic::One);
        assert_eq!(bits.as_u128(), 0b0011);
    }

    #[test]
    fn test_bits_is_valid() {
        let valid: Bits<4> = Bits::from_array([Logic::Zero, Logic::One, Logic::One, Logic::Zero]);
        let invalid: Bits<4> = Bits::from_array([Logic::Zero, Logic::X, Logic::One, Logic::Zero]);

        assert!(valid.is_valid());
        assert!(!invalid.is_valid());
    }

    #[test]
    fn test_bits_shift_left() {
        let bits: Bits<8> = Bits::from_lit::<0b1011_0011>();

        let shifted = bits.shift_left(2);

        assert_eq!(shifted.as_u128(), 0b1100_1100);
    }

    #[test]
    fn test_bits_shift_right() {
        let bits: Bits<8> = Bits::from_lit::<0b1011_0011>();

        let shifted = bits.shift_right(3);

        assert_eq!(shifted.as_u128(), 0b0001_0110);
    }

    #[test]
    fn test_bits_with_lsb_and_msb() {
        let bits: Bits<4> = Bits::zero();

        let with_lsb = bits.with_lsb(Logic::One);
        let with_msb = bits.with_msb(Logic::X);

        assert_eq!(with_lsb.as_array(), &[Logic::One, Logic::Zero, Logic::Zero, Logic::Zero]);
        assert_eq!(with_msb.as_array(), &[Logic::Zero, Logic::Zero, Logic::Zero, Logic::X]);
    }

    #[test]
    fn test_bits_not() {
        let bits: Bits<4> = Bits::from_array([Logic::Zero, Logic::One, Logic::X, Logic::Zero]);

        let inverted = !bits;

        assert_eq!(inverted.as_array(), &[Logic::One, Logic::Zero, Logic::X, Logic::One]);
    }

    #[test]
    fn test_bits_display_binary_hex_octal_and_debug() {
        let bits: Bits<8> = Bits::from_lit::<0b1010_1101>();

        assert_eq!(format!("{}", bits), "10101101");
        assert_eq!(format!("{:b}", bits), "10101101");
        assert_eq!(format!("{:x}", bits), "ad");
        assert_eq!(format!("{:X}", bits), "AD");
        assert_eq!(format!("{:o}", bits), "255");
        assert_eq!(format!("{:?}", bits), "Bits<8>(1010_1101)");
    }

    // ── Bitwise ops ───────────────────────────────────────────────────────────

    #[test]
    fn test_bits_bitwise_and() {
        let a: Bits<4> = Bits::from_lit::<0b1100>();
        let b: Bits<4> = Bits::from_lit::<0b1010>();
        assert_eq!((a & b).as_u128(), 0b1000);
    }

    #[test]
    fn test_bits_bitwise_and_x_propagation() {
        let a = Bits::from_array([Logic::X, Logic::X, Logic::Zero, Logic::One]);
        let b = Bits::from_array([Logic::Zero, Logic::One, Logic::One, Logic::One]);
        let r = a & b;
        // X & 0 = 0, X & 1 = X, 0 & 1 = 0, 1 & 1 = 1
        assert_eq!(r.as_array(), &[Logic::Zero, Logic::X, Logic::Zero, Logic::One]);
    }

    #[test]
    fn test_bits_bitwise_or() {
        let a: Bits<4> = Bits::from_lit::<0b1100>();
        let b: Bits<4> = Bits::from_lit::<0b1010>();
        assert_eq!((a | b).as_u128(), 0b1110);
    }

    #[test]
    fn test_bits_bitwise_or_x_propagation() {
        let a = Bits::from_array([Logic::X, Logic::X, Logic::Zero, Logic::One]);
        let b = Bits::from_array([Logic::Zero, Logic::One, Logic::One, Logic::Zero]);
        let r = a | b;
        // X | 0 = X, X | 1 = 1, 0 | 1 = 1, 1 | 0 = 1
        assert_eq!(r.as_array(), &[Logic::X, Logic::One, Logic::One, Logic::One]);
    }

    #[test]
    fn test_bits_bitwise_xor() {
        let a: Bits<4> = Bits::from_lit::<0b1100>();
        let b: Bits<4> = Bits::from_lit::<0b1010>();
        assert_eq!((a ^ b).as_u128(), 0b0110);
    }

    #[test]
    fn test_bits_bitwise_xor_x_propagation() {
        let a = Bits::from_array([Logic::X, Logic::Zero]);
        let b = Bits::from_array([Logic::One, Logic::Zero]);
        let r = a ^ b;
        assert_eq!(r.as_array(), &[Logic::X, Logic::Zero]);
    }

    #[test]
    fn test_bits_bitwise_assign_ops() {
        let mut a: Bits<4> = Bits::from_lit::<0b1111>();
        a &= Bits::from_lit::<0b1010>();
        assert_eq!(a.as_u128(), 0b1010);

        let mut b: Bits<4> = Bits::from_lit::<0b0000>();
        b |= Bits::from_lit::<0b1010>();
        assert_eq!(b.as_u128(), 0b1010);

        let mut c: Bits<4> = Bits::from_lit::<0b1111>();
        c ^= Bits::from_lit::<0b1010>();
        assert_eq!(c.as_u128(), 0b0101);
    }

    // ── Arithmetic ops ────────────────────────────────────────────────────────

    #[test]
    fn test_bits_add_basic() {
        let a: Bits<8> = Bits::from_u8(10);
        let b: Bits<8> = Bits::from_u8(20);
        assert_eq!((a + b).as_u128(), 30);
    }

    #[test]
    fn test_bits_add_wrapping() {
        let a: Bits<8> = Bits::from_u8(200);
        let b: Bits<8> = Bits::from_u8(100);
        assert_eq!((a + b).as_u128(), 44); // 300 mod 256
    }

    #[test]
    fn test_bits_add_x_propagation() {
        let a: Bits<8> = Bits::x();
        let b: Bits<8> = Bits::from_lit::<1>();
        assert_eq!(a + b, Bits::x());
    }

    #[test]
    fn test_bits_sub_basic() {
        let a: Bits<8> = Bits::from_lit::<30>();
        let b: Bits<8> = Bits::from_lit::<10>();
        assert_eq!((a - b).as_u128(), 20);
    }

    #[test]
    fn test_bits_sub_wrapping() {
        let a: Bits<8> = Bits::from_lit::<0>();
        let b: Bits<8> = Bits::from_lit::<1>();
        assert_eq!((a - b).as_u128(), 255);
    }

    #[test]
    fn test_bits_mul() {
        let a: Bits<8> = Bits::from_lit::<6>();
        let b: Bits<8> = Bits::from_lit::<7>();
        assert_eq!((a * b).as_u128(), 42);
    }

    #[test]
    fn test_bits_mul_wrapping() {
        let a: Bits<8> = Bits::from_lit::<200>();
        let b: Bits<8> = Bits::from_lit::<2>();
        assert_eq!((a * b).as_u128(), 144); // 400 mod 256
    }

    #[test]
    fn test_bits_div() {
        let a: Bits<8> = Bits::from_lit::<42>();
        let b: Bits<8> = Bits::from_lit::<6>();
        assert_eq!((a / b).as_u128(), 7);
    }

    #[test]
    fn test_bits_div_by_zero_is_x() {
        let a: Bits<8> = Bits::from_lit::<42>();
        let b: Bits<8> = Bits::from_lit::<0>();
        assert_eq!(a / b, Bits::x());
    }

    #[test]
    fn test_bits_rem() {
        let a: Bits<8> = Bits::from_lit::<17>();
        let b: Bits<8> = Bits::from_lit::<5>();
        assert_eq!((a % b).as_u128(), 2);
    }

    #[test]
    fn test_bits_rem_by_zero_is_x() {
        let a: Bits<8> = Bits::from_lit::<17>();
        let b: Bits<8> = Bits::from_lit::<0>();
        assert_eq!(a % b, Bits::x());
    }

    #[test]
    fn test_bits_neg() {
        let a: Bits<8> = Bits::from_lit::<1>();
        assert_eq!((-a).as_u128(), 255); // two's complement: -1 mod 256
        let b: Bits<8> = Bits::from_lit::<0>();
        assert_eq!((-b).as_u128(), 0);
    }

    #[test]
    fn test_bits_neg_x_propagation() {
        let a: Bits<8> = Bits::x();
        assert_eq!(-a, Bits::x());
    }

    // ── Shift ops ─────────────────────────────────────────────────────────────

    #[test]
    fn test_bits_shl_operator() {
        let a: Bits<8> = Bits::from_lit::<0b0000_0001>();
        assert_eq!((a << 3).as_u128(), 0b0000_1000);
    }

    #[test]
    fn test_bits_shr_operator() {
        let a: Bits<8> = Bits::from_lit::<0b1000_0000>();
        assert_eq!((a >> 3).as_u128(), 0b0001_0000);
    }

    #[test]
    fn test_bits_arithmetic_shift_right_positive() {
        // MSB = 0 (positive), fills with 0
        let a: Bits<8> = Bits::from_lit::<0b0100_0000>();
        assert_eq!(a.arithmetic_shift_right(2).as_u128(), 0b0001_0000);
    }

    #[test]
    fn test_bits_arithmetic_shift_right_negative() {
        // MSB = 1 (negative), fills with 1
        let a: Bits<8> = Bits::from_lit::<0b1000_0000>();
        assert_eq!(a.arithmetic_shift_right(2).as_u128(), 0b1110_0000);
    }

    #[test]
    fn test_bits_arithmetic_shift_right_x_msb() {
        // MSB is X, fills with X
        let a = Bits::from_array([Logic::Zero, Logic::Zero, Logic::Zero, Logic::X]);
        let r = a.arithmetic_shift_right(1);
        assert_eq!(r.as_array(), &[Logic::Zero, Logic::Zero, Logic::X, Logic::X]);
    }

    // ── Reductions ────────────────────────────────────────────────────────────

    #[test]
    fn test_and_reduce() {
        assert_eq!(Bits::<4>::all_ones().and_reduce(), Logic::One);
        assert_eq!(Bits::<4>::from_lit::<0b1110>().and_reduce(), Logic::Zero);
        // X & 1 & 1 & 1 = X
        let x1 = Bits::from_array([Logic::X, Logic::One, Logic::One, Logic::One]);
        assert_eq!(x1.and_reduce(), Logic::X);
        // X & 0 & 1 & 1 = Zero (zero dominates)
        let x0 = Bits::from_array([Logic::X, Logic::Zero, Logic::One, Logic::One]);
        assert_eq!(x0.and_reduce(), Logic::Zero);
    }

    #[test]
    fn test_or_reduce() {
        assert_eq!(Bits::<4>::zero().or_reduce(), Logic::Zero);
        assert_eq!(Bits::<4>::from_lit::<0b0001>().or_reduce(), Logic::One);
        // X | 0 | 0 | 0 = X
        let x0 = Bits::from_array([Logic::X, Logic::Zero, Logic::Zero, Logic::Zero]);
        assert_eq!(x0.or_reduce(), Logic::X);
        // X | 1 | 0 | 0 = One (one dominates)
        let x1 = Bits::from_array([Logic::X, Logic::One, Logic::Zero, Logic::Zero]);
        assert_eq!(x1.or_reduce(), Logic::One);
    }

    #[test]
    fn test_xor_reduce() {
        assert_eq!(Bits::<4>::from_lit::<0b0110>().xor_reduce(), Logic::Zero); // even parity
        assert_eq!(Bits::<4>::from_lit::<0b0111>().xor_reduce(), Logic::One);  // odd parity
        let xv = Bits::from_array([Logic::X, Logic::Zero, Logic::Zero, Logic::Zero]);
        assert_eq!(xv.xor_reduce(), Logic::X);
    }

    #[test]
    fn test_nand_nor_xnor_reduce() {
        assert_eq!(Bits::<4>::all_ones().nand_reduce(), Logic::Zero);
        assert_eq!(Bits::<4>::zero().nor_reduce(), Logic::One);
        assert_eq!(Bits::<4>::from_lit::<0b0110>().xnor_reduce(), Logic::One); // even parity → xnor = 1
    }

    // ── X-aware comparisons ───────────────────────────────────────────────────

    #[test]
    fn test_eq_logic() {
        let a: Bits<8> = Bits::from_lit::<42>();
        assert_eq!(a.eq_logic(&Bits::from_lit::<42>()), Logic::One);
        assert_eq!(a.eq_logic(&Bits::from_lit::<43>()), Logic::Zero);
        assert_eq!(a.eq_logic(&Bits::x()), Logic::X);
        assert_eq!(Bits::<8>::x().eq_logic(&Bits::from_lit::<42>()), Logic::X);
    }

    #[test]
    fn test_ne_logic() {
        let a: Bits<8> = Bits::from_lit::<42>();
        assert_eq!(a.ne_logic(&Bits::from_lit::<43>()), Logic::One);
        assert_eq!(a.ne_logic(&Bits::from_lit::<42>()), Logic::Zero);
        assert_eq!(a.ne_logic(&Bits::x()), Logic::X);
    }

    #[test]
    fn test_lt_le_gt_ge_logic() {
        let five: Bits<8> = Bits::from_lit::<5>();
        let ten: Bits<8>  = Bits::from_lit::<10>();
        assert_eq!(five.lt_logic(&ten), Logic::One);
        assert_eq!(ten.lt_logic(&five), Logic::Zero);
        assert_eq!(five.lt_logic(&five), Logic::Zero);
        assert_eq!(five.le_logic(&five), Logic::One);
        assert_eq!(ten.gt_logic(&five), Logic::One);
        assert_eq!(five.ge_logic(&five), Logic::One);
        assert_eq!(five.lt_logic(&Bits::x()), Logic::X);
    }

    // ── Width conversion ──────────────────────────────────────────────────────

    #[test]
    fn test_zero_extend() {
        let a: Bits<4> = Bits::from_lit::<0b1010>();
        let b: Bits<8> = a.zero_extend();
        assert_eq!(b.as_u128(), 0b1010);
        assert_eq!(format!("{}", b), "00001010");
    }

    #[test]
    fn test_sign_extend_positive() {
        // MSB = 0, sign bit = 0 → fills with zeros
        let a: Bits<4> = Bits::from_lit::<0b0101>(); // 5
        let b: Bits<8> = a.sign_extend();
        assert_eq!(b.as_u128(), 5);
    }

    #[test]
    fn test_sign_extend_negative() {
        // MSB = 1 (0b1101 = -3 in 4-bit two's complement)
        let a: Bits<4> = Bits::from_lit::<0b1101>();
        let b: Bits<8> = a.sign_extend();
        assert_eq!(b.as_u128(), 0b1111_1101); // -3 in 8-bit
    }

    #[test]
    fn test_sign_extend_x_msb() {
        let a = Bits::from_array([Logic::Zero, Logic::Zero, Logic::Zero, Logic::X]);
        let b: Bits<6> = a.sign_extend();
        assert_eq!(b.as_array()[4], Logic::X);
        assert_eq!(b.as_array()[5], Logic::X);
    }

    #[test]
    fn test_truncate() {
        let a: Bits<8> = Bits::from_lit::<0b1010_0101>();
        let b: Bits<4> = a.truncate();
        assert_eq!(b.as_u128(), 0b0101); // keeps LSBs
    }

    // ── Part select, concat, replicate ───────────────────────────────────────

    #[test]
    fn test_part_select() {
        let a: Bits<8> = Bits::from_lit::<0b1010_0110>();
        // bits [2..+4) = bits 2,3,4,5 = 1001
        let s: Bits<4> = a.part_select(2);
        assert_eq!(s.as_u128(), 0b1001);
    }

    #[test]
    fn test_concat() {
        // {0b1010, 0b0101} = 0b1010_0101
        let hi: Bits<4> = Bits::from_lit::<0b1010>();
        let lo: Bits<4> = Bits::from_lit::<0b0101>();
        let result: Bits<8> = hi.concat(&lo);
        assert_eq!(result.as_u128(), 0b1010_0101);
    }

    #[test]
    fn test_concat_preserves_x() {
        let hi = Bits::from_array([Logic::One, Logic::X]);
        let lo = Bits::from_array([Logic::Zero, Logic::One]);
        let result: Bits<4> = hi.concat(&lo);
        assert_eq!(result.as_array(), &[Logic::Zero, Logic::One, Logic::One, Logic::X]);
    }

    #[test]
    fn test_replicate() {
        let a: Bits<4> = Bits::from_lit::<0b1010>();
        let r: Bits<8> = a.replicate();
        assert_eq!(r.as_u128(), 0b1010_1010);
    }

    // ── Mux ──────────────────────────────────────────────────────────────────

    #[test]
    fn test_mux_one_selects_a() {
        let a: Bits<4> = Bits::from_lit::<0b1010>();
        let b: Bits<4> = Bits::from_lit::<0b0101>();
        assert_eq!(Bits::mux(Logic::One, &a, &b), a);
    }

    #[test]
    fn test_mux_zero_selects_b() {
        let a: Bits<4> = Bits::from_lit::<0b1010>();
        let b: Bits<4> = Bits::from_lit::<0b0101>();
        assert_eq!(Bits::mux(Logic::Zero, &a, &b), b);
    }

    #[test]
    fn test_mux_x_matching_bits_pass_through() {
        // a = 0b1110, b = 0b1010 (LSB-first: [0,1,1,1] vs [0,1,0,1])
        let a: Bits<4> = Bits::from_lit::<0b1110>();
        let b: Bits<4> = Bits::from_lit::<0b1010>();
        let r = Bits::mux(Logic::X, &a, &b);
        // bit0: 0==0 → 0, bit1: 1==1 → 1, bit2: 1!=0 → X, bit3: 1==1 → 1
        assert_eq!(r.as_array(), &[Logic::Zero, Logic::One, Logic::X, Logic::One]);
    }

    #[test]
    fn test_mux_x_all_differ() {
        let a: Bits<4> = Bits::all_ones();
        let b: Bits<4> = Bits::zero();
        let r = Bits::mux(Logic::X, &a, &b);
        assert_eq!(r, Bits::x());
    }

    // ── Conversions ───────────────────────────────────────────────────────────

    #[test]
    fn test_logic_to_bits1_roundtrip() {
        let l = Logic::One;
        let b: Bits<1> = Bits::from(l);
        let back: Logic = Logic::from(b);
        assert_eq!(back, Logic::One);

        let x: Bits<1> = Bits::from(Logic::X);
        assert_eq!(Logic::from(x), Logic::X);
    }

    #[test]
    fn test_from_u128_for_bits() {
        let b: Bits<8> = Bits::from_lit::<42>();
        assert_eq!(b.as_u128(), 42);
    }
}
