//! File-scope `const` items → SystemVerilog `localparam`s.
//!
//! A Rust `const WIDTH: usize = 8;` sitting next to a `#[hardware]` module is
//! visible to that module's body and to its port types, but it is not reachable
//! from the `ItemFn` alone — `capture_file_scope` collects it into
//! `FrontendModuleIR::file_consts` and this pass turns the usable ones into
//! module-level constants.
//!
//! Three decisions worth knowing:
//!
//! * **`localparam`, not `parameter`.** A Rust `const` is a fixed value, not a
//!   knob; making it overridable at instantiation would let a synthesized module
//!   take a width no Copper simulation ever ran.
//! * **In the parameter port list, not the body.** A const may appear in a port
//!   width (`In<Bits<WIDTH>, D>` emits `[WIDTH-1:0]`), and a body declaration is
//!   not in scope there. SystemVerilog allows `local_parameter_declaration`
//!   inside a `parameter_port_list`; verified lint-clean against Verilator 5.044.
//! * **The source expression is preserved**, not an evaluated number, so
//!   `const MOD: usize = 1 << PTR_W` stays legible as
//!   `localparam int MOD = 1 << PTR_W`. That is also why this pass needs no
//!   const evaluator: SystemVerilog evaluates the expression itself.
//!
//! A const this pass cannot express (a `const fn` call, a non-integer type) is
//! *not* emitted. Referencing one is then the ordinary "undefined variable"
//! error, with a note explaining which const was skipped and why — see
//! [`rejection_note`].

use copper_core::chir::ModuleLocalParam;
use copper_core::frontend_ir::{FrontendModuleIR, ItemConst};

/// Integer types a `localparam int` can stand in for. A const of any other type
/// (`bool`, an array, a struct) is not a width or a bound and is skipped.
const INT_TYPES: &[&str] = &[
    "usize", "isize", "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64",
];

/// Every file-scope const the module *could* use, as `localparam`s in dependency
/// order. Consts that cannot be expressed in SystemVerilog are omitted.
///
/// This is the candidate set, not the emitted set: which ones actually reach the
/// output is decided at emission, from the rendered SystemVerilog, because an
/// unused `localparam` is a Verilator `UNUSEDPARAM` error under `-Wall`.
pub fn candidates(fir: &FrontendModuleIR) -> Vec<ModuleLocalParam> {
    let usable: Vec<(&ItemConst, String)> = fir
        .file_consts
        .iter()
        .filter(|c| is_int_type(&c.ty.ty_text))
        .filter_map(|c| sv_expr(&c.value_text).map(|sv| (c, sv)))
        .collect();

    let names: Vec<&str> = usable.iter().map(|(c, _)| c.name.as_str()).collect();
    let ordered = dependency_order(&usable, &names);

    ordered
        .into_iter()
        .map(|(c, sv)| ModuleLocalParam { name: c.name.clone(), value_expr: sv })
        .collect()
}

/// Why a referenced name that *is* a file-scope const did not become a
/// `localparam`. `None` when `name` is not a file-scope const at all (an
/// ordinary undefined variable).
pub fn rejection_note(fir: &FrontendModuleIR, name: &str) -> Option<String> {
    let c = fir.file_consts.iter().find(|c| c.name == name)?;
    if !is_int_type(&c.ty.ty_text) {
        return Some(format!(
            "`const {}: {}` is not an integer constant, so it has no SystemVerilog \
             `localparam` form",
            c.name,
            c.ty.ty_text.trim()
        ));
    }
    Some(format!(
        "`const {}` has an initializer the transpiler cannot express in \
         SystemVerilog (`{}`) — only integer literals, other file-scope integer \
         consts, and arithmetic/bitwise operators on them are supported. A \
         `const fn` call is evaluated by rustc, and nothing of it survives into \
         the emitted module",
        c.name,
        c.value_text.trim()
    ))
}

fn is_int_type(ty_text: &str) -> bool {
    let t: String = ty_text.chars().filter(|c| !c.is_whitespace()).collect();
    INT_TYPES.contains(&t.as_str())
}

/// Render a const initializer as SystemVerilog, or `None` if it has no such form.
///
/// Deliberately narrow: integer literals (Rust type suffixes stripped), bare
/// identifiers, parentheses, and the arithmetic/bitwise operators whose spelling
/// and meaning coincide in the two languages. A call, a path (`Foo::BAR`), a
/// cast, an index, or a method rejects the whole const rather than emitting text
/// that only looks like SystemVerilog.
fn sv_expr(value_text: &str) -> Option<String> {
    let expr: syn::Expr = syn::parse_str(value_text).ok()?;
    render(&expr)
}

fn render(expr: &syn::Expr) -> Option<String> {
    use syn::{BinOp, Expr, UnOp};
    match expr {
        Expr::Lit(l) => match &l.lit {
            // `base10_digits` drops the Rust suffix: `8usize` → `8`.
            syn::Lit::Int(i) => Some(i.base10_digits().to_string()),
            _ => None,
        },
        Expr::Path(p) => {
            let ident = p.path.get_ident()?;
            Some(ident.to_string())
        }
        Expr::Paren(p) => Some(format!("({})", render(&p.expr)?)),
        Expr::Group(g) => render(&g.expr),
        Expr::Unary(u) => {
            let op = match u.op {
                UnOp::Not(_) => "~", // Rust `!` on an integer is SystemVerilog `~`
                UnOp::Neg(_) => "-",
                _ => return None,
            };
            Some(format!("{}{}", op, render(&u.expr)?))
        }
        Expr::Binary(b) => {
            let op = match b.op {
                BinOp::Add(_) => "+",
                BinOp::Sub(_) => "-",
                BinOp::Mul(_) => "*",
                BinOp::Div(_) => "/",
                BinOp::Rem(_) => "%",
                BinOp::Shl(_) => "<<",
                BinOp::Shr(_) => ">>",
                BinOp::BitAnd(_) => "&",
                BinOp::BitOr(_) => "|",
                BinOp::BitXor(_) => "^",
                _ => return None,
            };
            Some(format!("{} {} {}", render(&b.left)?, op, render(&b.right)?))
        }
        _ => None,
    }
}

/// Order the consts so none precedes one it references. SystemVerilog resolves a
/// parameter port list left to right, so `MOD = 1 << PTR_W` must come after
/// `PTR_W`. A reference cycle is impossible (rustc rejects it), but the walk is
/// cycle-safe regardless: a const already being visited is simply not re-entered.
fn dependency_order<'a>(
    usable: &[(&'a ItemConst, String)],
    names: &[&str],
) -> Vec<(&'a ItemConst, String)> {
    let mut out: Vec<(&ItemConst, String)> = Vec::new();
    let mut placed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut visiting: std::collections::HashSet<String> = std::collections::HashSet::new();

    fn visit<'a>(
        idx: usize,
        usable: &[(&'a ItemConst, String)],
        names: &[&str],
        placed: &mut std::collections::HashSet<String>,
        visiting: &mut std::collections::HashSet<String>,
        out: &mut Vec<(&'a ItemConst, String)>,
    ) {
        let (c, sv) = &usable[idx];
        if placed.contains(&c.name) || !visiting.insert(c.name.clone()) {
            return;
        }
        for dep in referenced_idents(sv, names) {
            if let Some(j) = usable.iter().position(|(o, _)| o.name == dep) {
                visit(j, usable, names, placed, visiting, out);
            }
        }
        visiting.remove(&c.name);
        placed.insert(c.name.clone());
        out.push((*c, sv.clone()));
    }

    for i in 0..usable.len() {
        visit(i, usable, names, &mut placed, &mut visiting, &mut out);
    }
    out
}

/// The names from `names` that appear as whole identifiers in `text`.
pub fn referenced_idents(text: &str, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter(|n| contains_ident(text, n))
        .map(|n| n.to_string())
        .collect()
}

/// Whole-identifier containment: `WIDTH` is in `[WIDTH-1:0]` but not in
/// `MY_WIDTH` or `WIDTH_P`.
pub fn contains_ident(text: &str, ident: &str) -> bool {
    if ident.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(ident) {
        let start = from + rel;
        let end = start + ident.len();
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
