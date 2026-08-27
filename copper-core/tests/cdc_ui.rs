//! The CDC guarantees, asserted as compile errors with their messages pinned.
//!
//! `copper-core/src/cdc.rs` has no runtime code — the rules ARE the type system, so
//! the only way to test them is to write a program that must not compile. That used
//! to be six `compile_fail` doctests, and `compile_fail` is a weaker claim than it
//! looks: it passes when the snippet fails for ANY reason, so a typo, a renamed API
//! or a missing `use` keeps the test green while the guarantee it names quietly
//! stops being tested.
//!
//! trybuild pins the compiler's own output in a `.stderr` next to each case, so a
//! guarantee that starts failing for a different reason shows up as a diff.
//!
//! Regenerate the expectations after an intentional change:
//!
//! ```text
//! TRYBUILD=overwrite cargo test -p copper-core --test cdc_ui
//! ```
//!
//! and READ the diff — an unexpected change of error code is the signal this file
//! exists to produce.

#[test]
fn cdc_rules_are_rejected_at_compile_time() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/fail/*.rs");
}
