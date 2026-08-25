//! Array-typed ports (`In<[Bits<W>; ELS], D>`) — the packed-2-D ABI.
//!
//! The shape and the reasoning behind it are in
//! `design_docs/ARRAY_PORT_ABI.md`. In short: both independent BaseJump
//! references declare `[els_p-1:0][width_p-1:0]`, Verilator gives packed 2-D and
//! a flat `[ELS*W-1:0]` the identical C++ interface (so the testbench harness is
//! unaffected either way), and keeping the dimensions separate means neither
//! needs width *arithmetic* — which `Width` cannot express, and which both
//! blocked modules would have required since their dimensions are symbolic.
//!
//! `tests/mux_equivalence.rs` and `tests/bsg_mux_one_hot_equivalence.rs` at the
//! repo root carry the behavioural half under Verilator.

fn transpile(src: &str) -> Result<String, String> {
    copper_codegen::transpile_source(src, None, &copper_codegen::EmitConfig::default())
        .map_err(|e| e.to_string())
}

/// The declaration, at concrete dimensions: outer first, both packed.
#[test]
fn an_array_port_declares_both_packed_dimensions() {
    let src = r#"
#[hardware(combinational)]
fn mux4(data_i: In<[Bits<8>; 4], ()>, sel_i: In<Bits<2>, ()>, data_o: Out<Bits<8>, ()>) {
    data_o.write(data_i.read()[sel_i.read().as_u128() as usize]);
}
"#;
    let sv = transpile(src).expect("an array port must transpile");
    assert!(
        sv.contains("input  logic [3:0][7:0] data_i"),
        "expected a packed 2-D declaration, outer dimension first, got:\n{sv}"
    );
}

/// Symbolic dimensions — the case that decided the ABI. Neither dimension needs
/// arithmetic: each is independently a parameter reference, which is exactly what
/// a flat `[ELS_P*WIDTH_P-1:0]` could not have expressed.
#[test]
fn an_array_port_with_symbolic_dimensions_needs_no_width_arithmetic() {
    let src = r#"
#[hardware(combinational)]
fn mux<const WIDTH_P: usize, const ELS_P: usize, const LG_ELS_LP: usize>(
    data_i: In<[Bits<WIDTH_P>; ELS_P], ()>,
    sel_i: In<Bits<LG_ELS_LP>, ()>,
    data_o: Out<Bits<WIDTH_P>, ()>,
) {
    data_o.write(data_i.read()[sel_i.read().as_u128() as usize]);
}
"#;
    let sv = transpile(src).expect("symbolic dimensions must transpile");
    assert!(
        sv.contains("[ELS_P-1:0][WIDTH_P-1:0] data_i"),
        "each dimension must render as its own parameter reference, got:\n{sv}"
    );
    assert!(
        !sv.contains("ELS_P*WIDTH_P") && !sv.contains("ELS_P *"),
        "the packed-2-D ABI exists precisely so no width product is needed, got:\n{sv}"
    );
}

/// Indexing an array port is a direct element select, the same syntax the
/// references use — not an indexed part-select over a flattened vector.
#[test]
fn indexing_an_array_port_is_a_direct_element_select() {
    let src = r#"
#[hardware(combinational)]
fn mux4(data_i: In<[Bits<8>; 4], ()>, sel_i: In<Bits<2>, ()>, data_o: Out<Bits<8>, ()>) {
    data_o.write(data_i.read()[sel_i.read().as_u128() as usize]);
}
"#;
    let sv = transpile(src).expect("transpiles");
    assert!(sv.contains("data_i[sel_i]"), "expected a direct index, got:\n{sv}");
    assert!(!sv.contains("+:"), "a part-select means the flat ABI leaked in, got:\n{sv}");
}

/// A LOCAL holding an array needs the same two dimensions as the port. Declared
/// at the element width instead, `let d = data_i.read();` silently truncates the
/// array into a single element and every `d[i]` becomes a bit-select — the
/// emitted module compiles and is wrong. Found exactly that way on
/// `bsg_mux_one_hot` before this was threaded through.
#[test]
fn a_local_holding_an_array_keeps_both_dimensions() {
    let src = r#"
#[hardware(combinational)]
fn oh(data_i: In<[Bits<4>; 3], ()>, sel_i: In<Bits<3>, ()>, data_o: Out<Bits<4>, ()>) {
    let d = data_i.read();
    let sel = sel_i.read();
    let mut acc = Bits::<4>::zero();
    for i in 0..3 {
        if sel[i] == Logic::One {
            acc = acc | d[i];
        }
    }
    data_o.write(acc);
}
"#;
    let sv = transpile(src).expect("transpiles");
    assert!(
        sv.contains("logic [2:0][3:0] d;"),
        "an array local must declare both dimensions, got:\n{sv}"
    );
}

/// An array OF arrays would need a third packed dimension and has no instance in
/// the corpus. Refused rather than emitted as a shape nothing has verified.
#[test]
fn a_nested_array_port_is_refused() {
    let src = r#"
#[hardware(combinational)]
fn nested(data_i: In<[[Bits<4>; 2]; 3], ()>, data_o: Out<Bits<4>, ()>) {
    data_o.write(data_i.read()[0][0]);
}
"#;
    let err = transpile(src).expect_err("a nested array has no verified emission");
    assert!(err.contains("cannot resolve type"), "got: {err}");
}
