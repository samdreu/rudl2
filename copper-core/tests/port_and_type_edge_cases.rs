//! Behavioral coverage for the wire/port primitive and a few type width
//! boundaries not exercised by the `src/*.rs` unit tests.
//!
//! The `wire`/`In`/`Out` triple is the substrate every simulated design is wired
//! through, so its change-detection (`dirty`) and producer↔consumer identity
//! (`wire_id`) semantics are load-bearing for the executor's dependency graph.

use copper_core::port::{wire, WireKind};
use copper_core::types::Bits;
use copper_core::Logic;

// ── wire / port semantics ────────────────────────────────────────────────────

/// A value written to an `Out` is observable through the paired `In`.
#[test]
fn write_is_visible_through_paired_input() {
    let (out, inp) = wire::<Bits<8>, ()>(Bits::zero());
    assert_eq!(inp.read().as_u8(), 0);
    out.write(Bits::from_u8(0xA5));
    assert_eq!(inp.read().as_u8(), 0xA5);
}

/// A wire is born clean; a *changing* write dirties it; `take` consumes the flag.
#[test]
fn dirty_flag_tracks_changing_writes() {
    let (out, _inp) = wire::<Bits<8>, ()>(Bits::zero());
    let dh = out.dirty_handle();

    assert!(!dh.take(), "a fresh wire is not dirty");

    out.write(Bits::from_u8(1));
    assert!(dh.take(), "a value change sets dirty");
    assert!(!dh.take(), "take() clears the flag");
}

/// Writing the *same* value the wire already holds must NOT dirty it — this is
/// what lets the executor's fixpoint converge instead of oscillating forever.
#[test]
fn rewriting_the_same_value_does_not_dirty() {
    let (out, _inp) = wire::<Bits<8>, ()>(Bits::from_u8(7));
    let dh = out.dirty_handle();
    // Drain any initial state.
    dh.take();

    out.write(Bits::from_u8(7));
    assert!(!dh.take(), "re-writing the current value is a no-op for dirtiness");

    out.write(Bits::from_u8(8));
    assert!(dh.take(), "an actual change still dirties");
}

/// The `Out` and its paired `In` name the *same* wire; a `DirtyHandle` reports the
/// same identity. This is the producer↔consumer key the executor matches on.
#[test]
fn out_in_and_handle_share_one_wire_id() {
    let (out, inp) = wire::<Logic, ()>(Logic::Zero);
    assert_eq!(out.wire_id(), inp.wire_id(), "producer and consumer share a wire id");
    assert_eq!(out.dirty_handle().wire_id(), out.wire_id());
    assert_eq!(out.dirty_handle().wire_kind(), WireKind::Comb, "plain Out is combinational");
}

/// Independent wires have distinct identities.
#[test]
fn distinct_wires_have_distinct_ids() {
    let (a_out, _a_in) = wire::<Logic, ()>(Logic::Zero);
    let (b_out, _b_in) = wire::<Logic, ()>(Logic::Zero);
    assert_ne!(a_out.wire_id(), b_out.wire_id());
}

/// A cloned `In` observes writes through the original wire — many readers, one
/// writer.
#[test]
fn cloned_inputs_observe_the_same_wire() {
    let (out, inp) = wire::<Bits<4>, ()>(Bits::zero());
    let inp2 = inp.clone();
    out.write(Bits::from_usize(0xF));
    assert_eq!(inp.read().as_u8(), 0xF);
    assert_eq!(inp2.read().as_u8(), 0xF, "the clone sees the same wire");
}

// ── type width boundaries ────────────────────────────────────────────────────

/// The widest supported lane roundtrips through `from_u128`/`as_u128`.
#[test]
fn bits128_roundtrips_full_width() {
    let v: u128 = 0xDEAD_BEEF_0000_1111_2222_3333_4444_5555;
    let b = Bits::<128>::from_u128(v);
    assert_eq!(b.as_u128(), v);
}

/// `concat` is Verilog `{self, other}` — `self` lands in the high bits, `other`
/// in the low bits — and `part_select` recovers each half.
#[test]
fn concat_then_part_select_recovers_each_half() {
    let hi = Bits::<4>::from_usize(0b1010);
    let lo = Bits::<4>::from_usize(0b0110);
    let joined: Bits<8> = hi.concat::<4, 8>(&lo);
    assert_eq!(joined.as_u8(), 0b1010_0110, "self ({{hi}}) occupies the MSBs");
    let low: Bits<4> = joined.part_select::<4>(0);
    assert_eq!(low.as_u8(), 0b0110);
    let high: Bits<4> = joined.part_select::<4>(4);
    assert_eq!(high.as_u8(), 0b1010);
}

/// Shifting by the full width (or more) clears every bit.
#[test]
fn shift_by_full_width_clears() {
    let b = Bits::<8>::from_u8(0xFF);
    assert_eq!(b.shift_left(8).as_u8(), 0);
    assert_eq!(b.shift_right(8).as_u8(), 0);
    assert_eq!(b.shift_left(100).as_u8(), 0);
}

/// Arithmetic shift right replicates the sign (MSB) into vacated positions.
#[test]
fn arithmetic_shift_right_fills_with_sign() {
    // 0b1000_0000 (negative): asr 3 → 0b1111_0000.
    let neg = Bits::<8>::from_u8(0b1000_0000);
    assert_eq!(neg.arithmetic_shift_right(3).as_u8(), 0b1111_0000);
    // 0b0100_0000 (positive): asr 3 → 0b0000_1000.
    let pos = Bits::<8>::from_u8(0b0100_0000);
    assert_eq!(pos.arithmetic_shift_right(3).as_u8(), 0b0000_1000);
}

/// `replicate` tiles a narrow value across a wider one.
#[test]
fn replicate_tiles_the_pattern() {
    let one = Bits::<2>::from_usize(0b10);
    let wide: Bits<8> = one.replicate::<8>();
    assert_eq!(wide.as_u8(), 0b10_10_10_10);
}

/// `Bits<1>` ↔ `Logic` roundtrips in both directions.
#[test]
fn bits1_logic_roundtrip_both_ways() {
    assert_eq!(Logic::from(Bits::<1>::from(Logic::One)), Logic::One);
    assert_eq!(Logic::from(Bits::<1>::from(Logic::Zero)), Logic::Zero);
    let b: Bits<1> = Logic::One.into();
    assert_eq!(b.get(0), Logic::One);
}

/// An X anywhere in either operand makes an X-aware equality X (Verilog `===`-ish
/// pessimism), while two identical valid values compare One.
#[test]
fn eq_logic_is_x_pessimistic() {
    let a = Bits::<4>::from_usize(0b1010);
    let b = Bits::<4>::from_usize(0b1010);
    assert_eq!(a.eq_logic(&b), Logic::One);

    let mut x = Bits::<4>::from_usize(0b1010);
    x.set(1, Logic::X);
    assert_eq!(x.eq_logic(&b), Logic::X, "an X operand poisons equality");
}

/// Reduction operators over an all-ones and a mixed vector.
#[test]
fn reductions_on_known_vectors() {
    let ones = Bits::<4>::from_usize(0b1111);
    assert_eq!(ones.and_reduce(), Logic::One);
    assert_eq!(ones.or_reduce(), Logic::One);
    assert_eq!(ones.xor_reduce(), Logic::Zero); // even parity

    let mixed = Bits::<4>::from_usize(0b1011);
    assert_eq!(mixed.and_reduce(), Logic::Zero);
    assert_eq!(mixed.or_reduce(), Logic::One);
    assert_eq!(mixed.xor_reduce(), Logic::One); // odd parity
}
