//! Phase D — Verilog legalization: `SHIRModule` -> `VLIRModule`.
//!
//! A mechanical, semantics-preserving pass. See `design_docs/VLIR_DESIGN.md`.
//!
//! Implemented for Milestone 1: name legalization (D2), literal width
//! annotation (D3/D5), mux->ternary + case-expr lifting (D4), and multi-phase
//! guard injection (D6). Tuple-pattern match lowering and conditional output
//! drives are recognized-but-not-yet-supported and produce a clear error.

use std::collections::{HashMap, HashSet};

use copper_core::chir::{CHIRBinOp, CHIRType, CHIRUnOp, Width};
use copper_core::memory::WriteMode;
use copper_core::shir::{
    SHIRBody, SHIRCombBody, SHIRExpr, SHIRLit, SHIRMemInit, SHIRMemory, SHIRModule, SHIRPhase, SHIRPortDir, SHIRPortKind,
    SHIRRegUpdate, SHIRSeqBody, SHIRStmt, SHIRStructuralBody, SHIRSubmoduleInst,
};
use copper_core::vlir::{
    VLIRAlwaysFF, VLIRBinOp, VLIRCaseArm, VLIRBody, VLIRCombBody, VLIRCombPhase, VLIRContinuousAssign, VLIRExpr,
    VLIRFFCaseArm, VLIRFFStmt, VLIRMemDecl, VLIRMemInit, VLIRMemReadNet, VLIRModule, VLIRPort, VLIRPortDir, VLIRPortKind, VLIRRegDecl, VLIRSeqBody, VLIRStmt,
    VLIRStructuralBody, VLIRSubmoduleInst, VLIRUnOp,
};

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum VLIRLowerError {
    /// A `match` on a tuple scrutinee — deferred to M2 (see VLIR_DESIGN §Pass 2).
    TuplePatternUnsupported,
    /// An output port driven inside a conditional branch — deferred to M2.
    ConditionalOutputUnsupported { port: String },
    /// A `Case` expression nested inside another expression (only top-level
    /// case-expressions in reg updates / wire values are lifted for now).
    NestedCaseExpr,
    /// A non-concrete width reached Phase D — parametric widths are M2.
    SymbolicWidthUnsupported,
    /// A combinational signal is assigned on some control paths but not all, which
    /// would infer a latch. Copper exists to make this class of bug impossible, so
    /// it is rejected rather than emitted.
    LatchInferred { signals: Vec<String> },
    /// An output port is driven in more than one phase, which would emit multiple
    /// unguarded continuous assigns (`assign out = a; assign out = 0;`). Verilator
    /// does not flag this, so it is rejected until phase-muxed / registered outputs
    /// are decided (the conditional-output-semantics milestone).
    MultiplyDrivenOutput { port: String },
    /// A plain `Out` is driven from a memory read result in a multi-phase module.
    /// See `reject_memory_driven_comb_outputs` for why neither emitted form is
    /// correct.
    MemoryResultDrivesPlainOutput { port: String },
    /// A memory access reached the 1:1 statement lowering. It produces several
    /// statements, so it must go through `lower_comb_stmts`; reaching here means a
    /// new caller bypassed that.
    MemoryAccessOutOfLine,
}

impl std::fmt::Display for VLIRLowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VLIRLowerError::TuplePatternUnsupported =>
                write!(f, "tuple-pattern match lowering is not yet implemented (M2)"),
            VLIRLowerError::ConditionalOutputUnsupported { port } =>
                write!(f, "conditional drive of output port '{port}' is not yet implemented (M2)"),
            VLIRLowerError::NestedCaseExpr =>
                write!(f, "match-as-expression nested inside another expression is not yet implemented"),
            VLIRLowerError::SymbolicWidthUnsupported =>
                write!(f, "parametric/symbolic bit widths are not yet implemented (M2)"),
            VLIRLowerError::MemoryResultDrivesPlainOutput { port } => write!(
                f,
                "output port '{port}' is driven from a memory read result in a multi-phase \
                 module. The read pipeline re-captures on every clock edge, so a plain `Out` \
                 either tracks it into the phases that do not observe it, or holds one edge \
                 late — neither matches the simulator. Declare the port `RegOut<T, D>`, or \
                 latch the result into a register and drive the output from that"
            ),
            VLIRLowerError::MemoryAccessOutOfLine =>
                write!(f, "internal: a memory access was lowered outside `lower_comb_stmts`"),
            VLIRLowerError::LatchInferred { signals } =>
                write!(
                    f,
                    "would infer a latch: {} assigned on some control paths but not all. \
                     Assign on every path (add the missing branch/`_` arm), or make it a \
                     register by assigning it every cycle",
                    signals.join(", ")
                ),
            VLIRLowerError::MultiplyDrivenOutput { port } =>
                write!(
                    f,
                    "output port '{port}' is driven in more than one phase (across \
                     clk.tick().await boundaries), which would emit multiple conflicting \
                     drivers. Drive it in exactly one phase, or hold it in a register",
                ),
        }
    }
}

type LowerResult<T> = Result<T, VLIRLowerError>;

// ── Public entry point ──────────────────────────────────────────────────────

pub fn lower_to_vlir(shir: &SHIRModule) -> LowerResult<VLIRModule> {
    let mut leg = Legalizer::new();

    // Legalize all declared names up front so every reference resolves
    // consistently, regardless of body-traversal order.
    let name = leg.legalize(&shir.name);
    for p in &shir.ports {
        leg.legalize(&p.name);
    }
    collect_and_legalize_body_names(&shir.body, &mut leg);

    let ports: Vec<VLIRPort> = shir
        .ports
        .iter()
        .map(|p| VLIRPort {
            name: leg.get(&p.name),
            outer_dim: match &p.kind {
                SHIRPortKind::Clock => None,
                SHIRPortKind::Data { ty } => outer_dim_of(ty),
            },
            direction: match p.direction {
                SHIRPortDir::Input => VLIRPortDir::Input,
                SHIRPortDir::Output => VLIRPortDir::Output,
            },
            kind: match &p.kind {
                SHIRPortKind::Clock => VLIRPortKind::Clock,
                SHIRPortKind::Data { .. } => VLIRPortKind::Logic,
            },
            width: match &p.kind {
                SHIRPortKind::Clock => Width::Concrete(1),
                SHIRPortKind::Data { ty } => width_of(ty),
            },
            registered: p.registered,
        })
        .collect();

    // Output ports declared `RegOut<T,D>` are registered by construction (driven
    // from always_ff), independent of whether the body writes them on all paths.
    let registered_outs: HashSet<String> = shir
        .ports
        .iter()
        .filter(|p| p.registered)
        .map(|p| leg.get(&p.name))
        .collect();

    let body = match &shir.body {
        SHIRBody::Combinational(c) => VLIRBody::Combinational(lower_comb(c, &leg)?),
        SHIRBody::Sequential(s) => VLIRBody::Sequential(lower_seq(s, &leg, &registered_outs)?),
        SHIRBody::Structural(st) => VLIRBody::Structural(lower_structural(st, &leg)?),
    };

    let params: Vec<copper_core::vlir::VLIRParam> = shir
        .params
        .iter()
        .map(|p| copper_core::vlir::VLIRParam { name: p.name.clone(), default: p.default })
        .collect();

    // `localparam` names are already legal SystemVerilog identifiers (they come
    // from Rust consts, whose spelling rules are a subset), and unlike signals
    // they are not renamed by the legalizer — a port width referring to `WIDTH`
    // must still say `WIDTH`.
    let localparams = shir
        .localparams
        .iter()
        .map(|lp| copper_core::vlir::VLIRLocalParam {
            name: lp.name.clone(),
            value_expr: lp.value_expr.clone(),
        })
        .collect();

    // RECEIVED memories: synthesize the bus ports and the address-width
    // parameter (design_docs/RECEIVED_MEMORY_ABI.md). Per USED read port the bus
    // is `<m>_rd<i>_addr` out + `<m>_rd<i>_data` in (a continuous array read
    // needs no enable; the child's own valid pipeline handles readiness); per
    // used write port it is `<m>_wr<j>_{en,addr,data}` out. The names are the
    // SAME nets the body lowering already drives — the port declaration simply
    // replaces the internal wire declaration (the emitter skips redeclaring
    // port names).
    let mut ports = ports;
    let mut params = params;
    if let SHIRBody::Sequential(s) = &shir.body {
        let mut mem_use = MemPortUse::default();
        collect_phase_mem_use(&s.phases, &mut mem_use);
        for m in s.memories.iter().filter(|m| m.received) {
            params.push(copper_core::vlir::VLIRParam {
                name: addr_param_name(&m.name),
                default: Some(1),
            });
            let aw = Width::Param(addr_param_name(&m.name));
            let dw = width_of(&m.elem_ty);
            let bus = |name: String, dir: VLIRPortDir, width: Width| VLIRPort {
                name,
                outer_dim: None,
                direction: dir,
                kind: VLIRPortKind::Logic,
                width,
                registered: false,
            };
            for p in 0..m.read_ports {
                if !mem_use.reads.contains(&(m.name.clone(), p)) {
                    continue;
                }
                ports.push(bus(
                    leg.get(&mem_net(&m.name, true, p, "addr")),
                    VLIRPortDir::Output,
                    aw.clone(),
                ));
                ports.push(bus(
                    leg.get(&mem_net(&m.name, true, p, "data")),
                    VLIRPortDir::Input,
                    dw.clone(),
                ));
            }
            for wp in 0..m.write_ports {
                if !mem_use.writes.contains(&(m.name.clone(), wp)) {
                    continue;
                }
                ports.push(bus(
                    leg.get(&mem_net(&m.name, false, wp, "en")),
                    VLIRPortDir::Output,
                    Width::Concrete(1),
                ));
                ports.push(bus(
                    leg.get(&mem_net(&m.name, false, wp, "addr")),
                    VLIRPortDir::Output,
                    aw.clone(),
                ));
                ports.push(bus(
                    leg.get(&mem_net(&m.name, false, wp, "data")),
                    VLIRPortDir::Output,
                    dw.clone(),
                ));
            }
        }
    }

    let mut module = VLIRModule { name, params, localparams, ports, body };
    narrow_sole_resize_wires(&mut module);
    Ok(module)
}

// ── Combinational body ──────────────────────────────────────────────────────

fn lower_comb(c: &SHIRCombBody, leg: &Legalizer) -> LowerResult<VLIRCombBody> {
    let submodules = c.submodules.iter().map(|s| lower_submodule(s, leg, &MemBinding::new())).collect::<LowerResult<_>>()?;
    // A combinational module has no clock, so it has no registered outputs.
    let (mut comb_stmts, output_assigns) = lower_flat_stmts(&c.stmts, leg, &HashSet::new(), &MemWidths::default(), &MemBinding::new())?;
    // A combinational module has no registers, so nothing is exempt.
    hoist_branch_local_defaults(&mut comb_stmts, &HashSet::new());
    check_no_latches(&comb_stmts)?;
    Ok(VLIRCombBody { submodules, comb_stmts, output_assigns })
}

// ── Structural body ─────────────────────────────────────────────────────────

fn lower_structural(st: &SHIRStructuralBody, leg: &Legalizer) -> LowerResult<VLIRStructuralBody> {
    let nets = st.nets.iter().map(|(n, ty)| (leg.get(n), width_of(ty))).collect();
    let submodules = st.submodules.iter().map(|s| lower_submodule(s, leg, &MemBinding::new())).collect::<LowerResult<_>>()?;
    Ok(VLIRStructuralBody { nets, submodules })
}

// ── Sequential body ─────────────────────────────────────────────────────────

fn lower_seq(s: &SHIRSeqBody, leg: &Legalizer, registered_outs: &HashSet<String>) -> LowerResult<VLIRSeqBody> {
    let clock = leg.get(&s.clock);

    let mut reg_decls: Vec<VLIRRegDecl> = s
        .registers
        .iter()
        .map(|r| VLIRRegDecl { name: leg.get(&r.name), width: width_of(&r.ty) })
        .collect();

    let submodules = s.submodules.iter().map(|m| lower_submodule(m, leg, &MemBinding::new())).collect::<LowerResult<_>>()?;

    // Width of the phase register (for phase guards / PhaseEq literals).
    let phase_r_width = s
        .registers
        .iter()
        .find(|r| r.name == "phase_r")
        .map(|r| width_of(&r.ty))
        .unwrap_or(Width::Concrete(1));

    let multi_phase = s.phases.len() > 1;

    // Outputs that must behave as implicit-hold registers rather than wires:
    // explicitly-declared `RegOut` ports, plus plain `Out` ports written in some
    // phases but not all (see `phase_scoped_output_ports`). Both need their
    // drives kept in the phase's statements so `split_output_regs` can move them
    // into `always_ff` under the phase guard — a module-level continuous assign
    // would lose both the register and the guard.
    let hold_outs: HashSet<String> = registered_outs
        .iter()
        .cloned()
        .chain(phase_scoped_output_ports(&s.phases).into_iter().map(|p| leg.get(&p)))
        .collect();

    // Memory nets: widths for the accesses to drive, which ports are used at all
    // (an unused port of a multi-port memory gets no nets), and — for read results
    // — whether each observation reads the combinational value or the registered
    // capture. See `MemPortUse`.
    reject_memory_driven_comb_outputs(&s.phases, registered_outs, leg)?;

    let mut mem_use = MemPortUse::default();
    collect_phase_mem_use(&s.phases, &mut mem_use);
    let mem_widths = MemWidths::build(&s.memories, &mem_use);
    let memories = lower_mem_decls(&s.memories, &mem_use, leg)?;
    reg_decls.extend(mem_pipeline_regs(&s.memories, &mem_use, leg));

    let mut comb_phases = Vec::new();
    let mut output_assigns = Vec::new();
    let mut ff_stmts = Vec::new();

    for phase in &s.phases {
        let staged_here = staged_read_ports(&phase.pre_edge);
        // Pre-edge statements run after the capture edge, so a read result they
        // observe is the registered one.
        let pre_bind = mem_binding(&s.memories, &staged_here, false, leg);
        let (mut stmts, mut outs) =
            lower_flat_stmts(&phase.pre_edge, leg, &hold_outs, &mem_widths, &pre_bind)?;

        // Memory net defaults go first, so a port driven only inside an `if` is
        // still assigned on every path (no latch, and the enable reads as 0).
        // Scoped to what THIS phase drives; a multi-phase module gets the
        // cross-phase clearing from the emitter's top-of-block defaults.
        let mut driven = MemPortUse::default();
        collect_mem_use(&phase.pre_edge, &mut driven);
        let mut defaults = mem_net_defaults(&s.memories, &driven, &mem_use, leg);
        if phase.phase_idx == 0 {
            defaults.extend(unstaged_read_defaults(&s.memories, &mem_use, leg));
        }
        stmts.splice(0..0, defaults);
        // Output continuous assigns are module-level; collect from every phase.
        output_assigns.append(&mut outs);

        // Keep a Moore output combinational when it is only "conditional" because
        // the state `case` has unreachable encodings (non-power-of-two state count,
        // or a wide state var): give it a top-of-block default so it is driven on
        // all paths, instead of letting the check below register it (one cycle late).
        hoist_moore_output_defaults(&mut stmts);

        // A conditionally-driven output holds between writes → an implicit-hold
        // register. Move its drives from `always_comb` (where an undriven path is
        // a latch) to `always_ff` (`if (guard) out <= v`, holding otherwise),
        // preserving the guard structure. Done before the latch check so the
        // registered output no longer counts as a comb latch.
        // Registered = conditionally-driven (implicit-hold, latch-avoidance) ∪
        // explicitly-declared `RegOut` ports (registered even when written on all
        // paths — an unconditional `RegOut` is a plain D flip-flop).
        let mut cond_outs = conditional_output_ports(&stmts);
        cond_outs.extend(hold_outs.iter().cloned());
        if !cond_outs.is_empty() {
            let (cleaned, out_ff) = split_output_regs(&stmts, &cond_outs);
            stmts = cleaned;
            if multi_phase {
                ff_stmts.push(VLIRFFStmt::If {
                    condition: phase_eq(phase.phase_idx, &phase_r_width),
                    then_stmts: out_ff,
                    else_stmts: None,
                });
            } else {
                ff_stmts.extend(out_ff);
            }
        }

        let phase_guard = if multi_phase {
            Some(phase_eq(phase.phase_idx, &phase_r_width))
        } else {
            None
        };
        // Registers and registered outputs are exempt: a conditionally-driven
        // register is the implicit-hold idiom (verified against BaseJump's
        // `bsg_dff_en`), not a latch bug, and an unconditional zero default
        // would clear it on every untaken path.
        let protected: HashSet<String> = reg_decls
            .iter()
            .map(|r| r.name.clone())
            .chain(registered_outs.iter().cloned())
            .collect();
        hoist_branch_local_defaults(&mut stmts, &protected);
        check_no_latches(&stmts)?;
        comb_phases.push(VLIRCombPhase { phase_guard, stmts });

        // Register updates -> always_ff non-blocking assigns, guarded by phase
        // for multi-phase modules. These latch at THIS phase's edge, so a read
        // staged in this phase is read combinationally (same edge) here.
        let post_bind = mem_binding(&s.memories, &staged_here, true, leg);
        let updates = lower_reg_updates(&phase.post_edge, leg, &post_bind)?;
        if multi_phase {
            ff_stmts.push(VLIRFFStmt::If {
                condition: phase_eq(phase.phase_idx, &phase_r_width),
                then_stmts: updates,
                else_stmts: None,
            });
        } else {
            ff_stmts.extend(updates);
        }
    }

    // A port driven in more than one phase accumulates multiple continuous
    // assigns to the same target — multiply-driven, undefined, and not flagged by
    // Verilator. Reject rather than emit it.
    let mut seen_ports = HashSet::new();
    for a in &output_assigns {
        if !seen_ports.insert(a.target.clone()) {
            return Err(VLIRLowerError::MultiplyDrivenOutput { port: a.target.clone() });
        }
    }

    // A `MemIndex` sampled in ALWAYS_FF position reads the array JUST BEFORE
    // the edge — one commit behind the simulator's statement-order read (the
    // pipelined CPU's halt-state `a0_out <= regs[10]` captured the register
    // file without the write committing at that same edge). Forward from each
    // write port's COMMITTING stage, exactly `read_net_value`'s WriteFirst
    // rule applied in edge position.
    forward_ff_mem_index(&mut ff_stmts, &s.memories, &mem_use, leg);

    // The read pipeline stage and the write commits close out `always_ff`;
    // nonblocking assignment makes their position relative to the register
    // updates immaterial.
    ff_stmts.extend(mem_read_pipeline(&s.memories, &mem_use, leg));
    // The commit reads the last write stage, so it must be emitted before the
    // shift that overwrites it — non-blocking assignment makes the order
    // immaterial for correctness, but keeping it reads the way the pipeline runs.
    ff_stmts.extend(mem_write_commits(&s.memories, &mem_use, leg));
    ff_stmts.extend(mem_write_pipeline(&s.memories, &mem_use, leg));

    // Now that every drive has committed to one of its two forms, a `let` wire whose
    // only reader was the discarded one is genuinely dead — and `-Wall` fails on it.
    let always_ff = VLIRAlwaysFF { clock: clock.clone(), stmts: ff_stmts };
    let mut comb_phases = comb_phases;
    // A RECEIVED memory's bus nets are read by the OWNER, not by anything in
    // this module (the array and the commit moved across the boundary), so the
    // dead-wire eliminator must treat them as externally read — without this it
    // silently deleted the bus's always_comb defaults on landing day.
    let mut external_reads: HashSet<String> = HashSet::new();
    for m in s.memories.iter().filter(|m| m.received) {
        for p in 0..m.read_ports {
            if mem_use.reads.contains(&(m.name.clone(), p)) {
                external_reads.insert(leg.get(&mem_net(&m.name, true, p, "addr")));
            }
        }
        for wp in 0..m.write_ports {
            if mem_use.writes.contains(&(m.name.clone(), wp)) {
                for suffix in ["en", "addr", "data"] {
                    external_reads.insert(leg.get(&mem_net(&m.name, false, wp, suffix)));
                }
            }
        }
    }
    drop_unread_wires(&mut comb_phases, &always_ff, &output_assigns, &memories, &external_reads);

    Ok(VLIRSeqBody {
        clock: clock.clone(),
        reg_decls,
        memories,
        submodules,
        comb_phases,
        always_ff,
        output_assigns,
    })
}

fn phase_eq(idx: usize, width: &Width) -> VLIRExpr {
    VLIRExpr::BinOp {
        left: Box::new(VLIRExpr::Var("phase_r".to_string())),
        op: VLIRBinOp::Eq,
        right: Box::new(VLIRExpr::Lit { width: width.clone(), value: idx as u128 }),
    }
}

fn lower_reg_updates(
    updates: &[SHIRRegUpdate],
    leg: &Legalizer,
    mb: &MemBinding,
) -> LowerResult<Vec<VLIRFFStmt>> {
    let mut out = Vec::new();
    for u in updates {
        let target = leg.get(&u.target);
        // Top-level case-as-expression is lifted into a case statement.
        if let SHIRExpr::Case { scrutinee, arms, default } = &u.next_value {
            let selector = lower_expr(scrutinee, leg, mb)?;
            let mut ff_arms = Vec::new();
            for arm in arms {
                let selector_value = pattern_to_selector(&arm.pattern, leg)?;
                ff_arms.push(copper_core::vlir::VLIRFFCaseArm {
                    selector_value,
                    stmts: vec![VLIRFFStmt::NonBlockingAssign {
                        target: target.clone(),
                        value: lower_expr(&arm.value, leg, mb)?,
                    }],
                });
            }
            out.push(VLIRFFStmt::Case {
                selector,
                arms: ff_arms,
                default: Some(vec![VLIRFFStmt::NonBlockingAssign {
                    target: target.clone(),
                    value: lower_expr(default, leg, mb)?,
                }]),
            });
        } else {
            out.push(VLIRFFStmt::NonBlockingAssign {
                target,
                value: lower_expr(&u.next_value, leg, mb)?,
            });
        }
    }
    Ok(out)
}

// ── Flat statement lowering (shared by comb + per-phase pre_edge) ────────────
//
// Returns (always_comb statements, module-level continuous output assigns).
// A top-level `PortDrive` becomes a continuous `assign`; a `PortDrive` nested
// inside `If`/`Match` is a conditional output (deferred to M2).

fn lower_flat_stmts(
    stmts: &[SHIRStmt],
    leg: &Legalizer,
    hold_outs: &HashSet<String>,
    mems: &MemWidths,
    mb: &MemBinding,
) -> LowerResult<(Vec<VLIRStmt>, Vec<VLIRContinuousAssign>)> {
    let mut comb = Vec::new();
    let mut assigns = Vec::new();
    for s in stmts {
        match s {
            SHIRStmt::Wire { name, ty, value } => comb.push(VLIRStmt::WireAssign {
                name: leg.get(name),
                width: width_of(ty),
                outer_dim: outer_dim_of(ty),
                value: lower_expr(value, leg, mb)?,
            }),
            // An UNCONDITIONAL write to an output that must HOLD — a `RegOut`,
            // or a plain `Out` written in only some phases — must still become a
            // flip-flop. Emitting it as a module-level continuous `assign` here
            // would put it out of reach of `split_output_regs` below, so the
            // hold would be silently dropped, and in a multi-phase module the
            // phase guard would be lost too: the output would follow a
            // phase-gated combinational value instead of holding. Keep it as a
            // `PortAssign` in the phase's statements and let the split move it
            // to `always_ff`.
            SHIRStmt::PortDrive { port_name, value, edge_value }
                if hold_outs.contains(&leg.get(port_name)) =>
            {
                comb.push(VLIRStmt::PortAssign {
                    port_name: leg.get(port_name),
                    value: lower_expr(value, leg, mb)?,
                    edge_value: lower_expr(edge_value, leg, mb)?,
                })
            }
            // Read as a continuous `assign`, i.e. AFTER the edge — so the plain,
            // unforwarded form is the correct one here. `edge_value` is dropped
            // deliberately: this drive never becomes a non-blocking assignment.
            SHIRStmt::PortDrive { port_name, value, .. } => assigns.push(VLIRContinuousAssign {
                target: leg.get(port_name),
                value: lower_expr(value, leg, mb)?,
            }),
            // Conditional structures — including ones that drive output ports
            // (a Moore output). These lower into `always_comb`, where the port is
            // assigned on every path, so no latch is inferred.
            SHIRStmt::If { .. } | SHIRStmt::Match { .. } | SHIRStmt::ForLoop { .. }
            | SHIRStmt::IndexAssign { .. } => {
                comb.push(lower_comb_stmt(s, leg, mems, mb)?);
            }
            // A memory access drives its port's nets; unconditionally here, or
            // under the enclosing `if` when it came from one.
            SHIRStmt::MemRead { .. } | SHIRStmt::MemWrite { .. } => {
                comb.extend(lower_mem_access(s, leg, mems, mb)?);
            }
        }
    }
    Ok((comb, assigns))
}

/// Lower a list of statements whose leaves are wire assignments or memory
/// accesses. One SHIR statement can produce several VLIR ones — a memory access
/// drives an enable, an address and (for a write) a data net — so the list form
/// is the primary entry point and `lower_comb_stmt` handles the 1:1 cases.
fn lower_comb_stmts(
    stmts: &[SHIRStmt],
    leg: &Legalizer,
    mems: &MemWidths,
    mb: &MemBinding,
) -> LowerResult<Vec<VLIRStmt>> {
    let mut out = Vec::new();
    for s in stmts {
        match s {
            SHIRStmt::MemRead { .. } | SHIRStmt::MemWrite { .. } => {
                out.extend(lower_mem_access(s, leg, mems, mb)?)
            }
            _ => out.push(lower_comb_stmt(s, leg, mems, mb)?),
        }
    }
    Ok(out)
}

/// The nets a single memory access drives, in `always_comb`.
fn lower_mem_access(
    s: &SHIRStmt,
    leg: &Legalizer,
    mems: &MemWidths,
    mb: &MemBinding,
) -> LowerResult<Vec<VLIRStmt>> {
    let one = Width::Concrete(1);
    let (mem, is_read, port, addr, value) = match s {
        SHIRStmt::MemRead { mem, port, addr } => (mem, true, *port, addr, None),
        SHIRStmt::MemWrite { mem, port, addr, value } => (mem, false, *port, addr, Some(value)),
        _ => return Ok(Vec::new()),
    };
    // The address net is sized to the array's depth, so a wider address
    // expression truncates here exactly as the simulator's `usize` index would
    // wrap — except the simulator panics on an out-of-range address instead, so
    // a design that reaches this truncation has already failed in simulation.
    //
    // That reasoning settles the SEMANTICS, and it always did; what it did not do
    // is tell Verilator. An address is almost always derived from a `usize`, which
    // is 32 bits, while the net is `clog2(depth)` — so the narrowing is implicit
    // and `-Wall` (fatal in `verification.rs`) rejects it as WIDTHTRUNC, however
    // well the design guards its own index. `rv32i_cpu_transpilable` is where this
    // stopped being theoretical: four addresses, every one already inside an
    // `if (idx < MEM_WORDS)`, and no source spelling could avoid the warning
    // (`truncate` / `part_select` do not lower, and a `Bits<10>` local still takes
    // a 32-bit right-hand side). So the cast is stated here, where the width is
    // known, rather than asked of every design that addresses a memory.
    //
    // UNCONDITIONAL, both directions. `W'(x)` on an already-`W`-bit expression is
    // a no-op, and a NARROWER address zero-extends — which is what the implicit
    // assignment did anyway, and silences the mirror-image WIDTHEXPAND. Making it
    // conditional would mean re-deriving the expression's width here, which is the
    // kind of second width calculation that drifts from the first.
    let addr_net = leg.get(&mem_net(mem, is_read, port, "addr"));
    let mut out = Vec::new();
    if !is_read || mems.wants_read_en(mem, port) {
        out.push(VLIRStmt::WireAssign {
            name: leg.get(&mem_net(mem, is_read, port, "en")),
            width: one.clone(),
            outer_dim: None, // a memory control net is never an array
            value: VLIRExpr::Lit { width: one, value: 1 },
        });
    }
    out.push(VLIRStmt::WireAssign {
        name: addr_net,
        width: mems.addr(mem),
        outer_dim: None,
        value: VLIRExpr::Resize {
            expr: Box::new(lower_expr(addr, leg, mb)?),
            width: mems.addr(mem),
        },
    });
    if let Some(v) = value {
        out.push(VLIRStmt::WireAssign {
            name: leg.get(&mem_net(mem, is_read, port, "data")),
            width: mems.data(mem),
            outer_dim: None,
            value: lower_expr(v, leg, mb)?,
        });
    }
    Ok(out)
}

/// Lower one statement that maps 1:1 onto a VLIR statement. Memory accesses do
/// not — they go through `lower_comb_stmts`, which is what every caller uses.
fn lower_comb_stmt(
    s: &SHIRStmt,
    leg: &Legalizer,
    mems: &MemWidths,
    mb: &MemBinding,
) -> LowerResult<VLIRStmt> {
    match s {
        SHIRStmt::Wire { name, ty, value } => Ok(VLIRStmt::WireAssign {
            name: leg.get(name),
            width: width_of(ty),
            outer_dim: outer_dim_of(ty),
            value: lower_expr(value, leg, mb)?,
        }),
        // A port driven inside a conditional becomes a blocking assign in
        // `always_comb` (the port is a `logic` output, assigned on every path).
        SHIRStmt::PortDrive { port_name, value, edge_value } => Ok(VLIRStmt::PortAssign {
            port_name: leg.get(port_name),
            value: lower_expr(value, leg, mb)?,
            edge_value: lower_expr(edge_value, leg, mb)?,
        }),
        SHIRStmt::If { condition, edge_condition, then_stmts, else_stmts } => Ok(VLIRStmt::If {
            condition: lower_expr(condition, leg, mb)?,
            edge_condition: lower_expr(edge_condition, leg, mb)?,
            then_stmts: lower_comb_stmts(then_stmts, leg, mems, mb)?,
            else_stmts: match else_stmts {
                Some(e) => Some(lower_comb_stmts(e, leg, mems, mb)?),
                None => None,
            },
        }),
        SHIRStmt::Match { scrutinee, edge_scrutinee, arms } => {
            let selector = lower_expr(scrutinee, leg, mb)?;
            let edge_selector = lower_expr(edge_scrutinee, leg, mb)?;
            let mut case_arms = Vec::new();
            let mut default = None;
            for arm in arms {
                let stmts = lower_comb_stmts(&arm.stmts, leg, mems, mb)?;
                // A bare wildcard arm becomes the `default` case.
                if arm.patterns.len() == 1
                    && matches!(arm.patterns[0], copper_core::shir::SHIRPattern::Wildcard)
                    && arm.guard.is_none()
                {
                    default = Some(stmts);
                } else {
                    for p in &arm.patterns {
                        case_arms.push(VLIRCaseArm {
                            selector_value: pattern_to_selector(p, leg)?,
                            stmts: stmts
                                .iter()
                                .map(|s| clone_comb_stmt(s))
                                .collect::<Vec<_>>(),
                        });
                    }
                }
            }
            Ok(VLIRStmt::Case { selector, edge_selector, arms: case_arms, default })
        }
        SHIRStmt::ForLoop { var, start, end, body } => Ok(VLIRStmt::ForLoop {
            var: var.clone(),
            start: lower_expr(start, leg, mb)?,
            end: lower_expr(end, leg, mb)?,
            body: lower_comb_stmts(body, leg, mems, mb)?,
        }),
        SHIRStmt::IndexAssign { base, index, value } => Ok(VLIRStmt::IndexAssign {
            base: leg.get(base),
            index: lower_expr(index, leg, mb)?,
            value: lower_expr(value, leg, mb)?,
        }),
        // Handled by `lower_comb_stmts`, the only caller that sees a list.
        SHIRStmt::MemRead { .. } | SHIRStmt::MemWrite { .. } => {
            Err(VLIRLowerError::MemoryAccessOutOfLine)
        }
    }
}

fn clone_comb_stmt(s: &VLIRStmt) -> VLIRStmt {
    match s {
        VLIRStmt::WireAssign { name, width, outer_dim, value } => VLIRStmt::WireAssign {
            name: name.clone(),
            width: width.clone(),
            outer_dim: outer_dim.clone(),
            value: value.clone(),
        },
        VLIRStmt::PortAssign { port_name, value, edge_value } => VLIRStmt::PortAssign {
            port_name: port_name.clone(),
            value: value.clone(),
            edge_value: edge_value.clone(),
        },
        VLIRStmt::If { condition, edge_condition, then_stmts, else_stmts } => VLIRStmt::If {
            condition: condition.clone(),
            edge_condition: edge_condition.clone(),
            then_stmts: then_stmts.iter().map(clone_comb_stmt).collect(),
            else_stmts: else_stmts
                .as_ref()
                .map(|e| e.iter().map(clone_comb_stmt).collect()),
        },
        VLIRStmt::Case { selector, edge_selector, arms, default } => VLIRStmt::Case {
            selector: selector.clone(),
            edge_selector: edge_selector.clone(),
            arms: arms
                .iter()
                .map(|a| VLIRCaseArm {
                    selector_value: a.selector_value.clone(),
                    stmts: a.stmts.iter().map(clone_comb_stmt).collect(),
                })
                .collect(),
            default: default.as_ref().map(|d| d.iter().map(clone_comb_stmt).collect()),
        },
        VLIRStmt::ForLoop { var, start, end, body } => VLIRStmt::ForLoop {
            var: var.clone(),
            start: start.clone(),
            end: end.clone(),
            body: body.iter().map(clone_comb_stmt).collect(),
        },
        VLIRStmt::IndexAssign { base, index, value } => VLIRStmt::IndexAssign {
            base: base.clone(),
            index: index.clone(),
            value: value.clone(),
        },
    }
}

// ── Submodule ───────────────────────────────────────────────────────────────

fn lower_submodule(
    m: &SHIRSubmoduleInst,
    leg: &Legalizer,
    mb: &MemBinding,
) -> LowerResult<VLIRSubmoduleInst> {
    Ok(VLIRSubmoduleInst {
        inst_name: leg.get(&m.inst_name),
        module_name: leg.get(&m.module_name),
        inputs: m
            .inputs
            .iter()
            .map(|(port, e)| Ok((leg.get(port), lower_expr(e, leg, mb)?)))
            .collect::<LowerResult<_>>()?,
        output_wire: leg.get(&m.output_wire),
        output_width: width_of(&m.output_ty),
        // Legalize both sides: child port names and the parent-side signal/net
        // names must match the identifiers emitted elsewhere in the module.
        clocks: m.clocks.iter().map(|(p, s)| (leg.get(p), leg.get(s))).collect(),
        port_nets: m.port_nets.iter().map(|(p, n)| (leg.get(p), leg.get(n))).collect(),
        output_port: m.output_port.as_ref().map(|p| leg.get(p)),
    })
}

// ── Expression lowering ─────────────────────────────────────────────────────

fn lower_expr(e: &SHIRExpr, leg: &Legalizer, mb: &MemBinding) -> LowerResult<VLIRExpr> {
    Ok(match e {
        SHIRExpr::Var(name) => VLIRExpr::Var(leg.get(name)),
        SHIRExpr::MemIndex { mem, addr } => VLIRExpr::MemIndex {
            mem: leg.get(mem),
            addr: Box::new(lower_expr(addr, leg, mb)?),
        },
        SHIRExpr::Lit(lit) => lower_lit(lit),
        // The read port's output, and its valid flag. WHICH net that is depends
        // on where this expression sits relative to the capture edge — see
        // `mem_binding`. Resolving it here rather than at the use site is what
        // keeps a single-tick post-tick read and a later-phase read both correct.
        SHIRExpr::MemData { mem, port } => VLIRExpr::Var(
            mb.get(&(mem.clone(), *port))
                .ok_or(VLIRLowerError::MemoryAccessOutOfLine)?
                .0
                .clone(),
        ),
        SHIRExpr::MemValid { mem, port } => VLIRExpr::Var(
            mb.get(&(mem.clone(), *port))
                .ok_or(VLIRLowerError::MemoryAccessOutOfLine)?
                .1
                .clone(),
        ),
        SHIRExpr::BinOp { left, op, right } => VLIRExpr::BinOp {
            left: Box::new(lower_expr(left, leg, mb)?),
            op: lower_binop(op),
            right: Box::new(lower_expr(right, leg, mb)?),
        },
        SHIRExpr::UnOp { op, expr } => VLIRExpr::UnOp {
            op: lower_unop(op),
            expr: Box::new(lower_expr(expr, leg, mb)?),
        },
        SHIRExpr::Mux { cond, then_val, else_val } => VLIRExpr::Ternary {
            cond: Box::new(lower_expr(cond, leg, mb)?),
            then_val: Box::new(lower_expr(then_val, leg, mb)?),
            else_val: Box::new(lower_expr(else_val, leg, mb)?),
        },
        SHIRExpr::Concat(parts) => {
            VLIRExpr::Concat(parts.iter().map(|p| lower_expr(p, leg, mb)).collect::<LowerResult<_>>()?)
        }
        SHIRExpr::Slice { expr, high, low } => VLIRExpr::Slice {
            expr: Box::new(lower_expr(expr, leg, mb)?),
            high: *high,
            low: *low,
        },
        SHIRExpr::DynBit { base, index } => VLIRExpr::DynBit {
            base: Box::new(lower_expr(base, leg, mb)?),
            index: Box::new(lower_expr(index, leg, mb)?),
        },
        SHIRExpr::Resize { expr, width } => VLIRExpr::Resize {
            expr: Box::new(lower_expr(expr, leg, mb)?),
            width: width.clone(),
        },
        SHIRExpr::SignCast { signed, expr } => VLIRExpr::SignCast {
            signed: *signed,
            expr: Box::new(lower_expr(expr, leg, mb)?),
        },
        SHIRExpr::PhaseEq(idx) => VLIRExpr::BinOp {
            left: Box::new(VLIRExpr::Var("phase_r".to_string())),
            op: VLIRBinOp::Eq,
            // Width filled by the phase register; 32 is a safe upper bound for a
            // bare comparison literal (SV zero-extends). Kept minimal here.
            right: Box::new(VLIRExpr::Lit { width: Width::Concrete(32), value: *idx as u128 }),
        },
        // A case used as a sub-expression (e.g. `Mux(cond, x, Case{…})` produced
        // by flattening `if … else { match … }`) becomes a ternary chain, which
        // composes anywhere an expression is allowed. A case at the *top* of a
        // register update is lifted to a `case` statement instead — see
        // `lower_reg_updates`.
        SHIRExpr::Case { scrutinee, arms, default } => {
            let sel = lower_expr(scrutinee, leg, mb)?;
            let mut result = lower_expr(default, leg, mb)?;
            // Fold from the last arm backwards so earlier arms take priority.
            for arm in arms.iter().rev() {
                let mut cond = VLIRExpr::BinOp {
                    left: Box::new(sel.clone()),
                    op: VLIRBinOp::Eq,
                    right: Box::new(pattern_to_selector(&arm.pattern, leg)?),
                };
                if let Some(g) = &arm.guard {
                    cond = VLIRExpr::BinOp {
                        left: Box::new(cond),
                        op: VLIRBinOp::LogicalAnd,
                        right: Box::new(lower_expr(g, leg, mb)?),
                    };
                }
                result = VLIRExpr::Ternary {
                    cond: Box::new(cond),
                    then_val: Box::new(lower_expr(&arm.value, leg, mb)?),
                    else_val: Box::new(result),
                };
            }
            result
        }
    })
}

fn lower_lit(lit: &SHIRLit) -> VLIRExpr {
    VLIRExpr::Lit { width: width_of(&lit.ty), value: lit.value }
}

fn lower_binop(op: &CHIRBinOp) -> VLIRBinOp {
    match op {
        // Fixed-width Verilog arithmetic already wraps; `wrapping` is a no-op here.
        CHIRBinOp::Add { .. } => VLIRBinOp::Add,
        CHIRBinOp::Sub { .. } => VLIRBinOp::Sub,
        CHIRBinOp::Mul { .. } => VLIRBinOp::Mul,
        CHIRBinOp::Rem => VLIRBinOp::Rem,
        CHIRBinOp::BitAnd => VLIRBinOp::BitAnd,
        CHIRBinOp::BitOr => VLIRBinOp::BitOr,
        CHIRBinOp::BitXor => VLIRBinOp::BitXor,
        CHIRBinOp::Shl => VLIRBinOp::Shl,
        CHIRBinOp::Shr => VLIRBinOp::Shr,
        CHIRBinOp::Eq => VLIRBinOp::Eq,
        CHIRBinOp::Neq => VLIRBinOp::Neq,
        CHIRBinOp::Lt => VLIRBinOp::Lt,
        CHIRBinOp::Lte => VLIRBinOp::Lte,
        CHIRBinOp::Gt => VLIRBinOp::Gt,
        CHIRBinOp::Gte => VLIRBinOp::Gte,
        CHIRBinOp::LogicalAnd => VLIRBinOp::LogicalAnd,
        CHIRBinOp::LogicalOr => VLIRBinOp::LogicalOr,
    }
}

fn lower_unop(op: &CHIRUnOp) -> VLIRUnOp {
    match op {
        CHIRUnOp::BitNot => VLIRUnOp::BitNot,
        CHIRUnOp::LogicalNot => VLIRUnOp::LogicalNot,
        CHIRUnOp::Neg => VLIRUnOp::Neg,
        CHIRUnOp::ReductionAnd => VLIRUnOp::ReductionAnd,
        CHIRUnOp::ReductionOr => VLIRUnOp::ReductionOr,
        CHIRUnOp::ReductionXor => VLIRUnOp::ReductionXor,
    }
}

/// Convert a scalar SHIR pattern into a case-selector expression.
/// Tuple patterns are deferred to M2.
fn pattern_to_selector(
    p: &copper_core::shir::SHIRPattern,
    leg: &Legalizer,
) -> LowerResult<VLIRExpr> {
    use copper_core::shir::SHIRPattern;
    match p {
        SHIRPattern::Lit(lit) => Ok(lower_lit(lit)),
        // A named constant (`localparam` / parameter) — the case label is the
        // NAME; SystemVerilog evaluates it (a case item may be any constant
        // expression).
        SHIRPattern::Const(name) => Ok(VLIRExpr::Var(leg.get(name))),
        // Tuple pattern → a single concatenated literal matching the `Concat`
        // scrutinee (VLIR_DESIGN §Pass 2). First element is most-significant.
        SHIRPattern::Tuple(_) => {
            let (width, value) = flatten_tuple_pattern(p)?;
            Ok(VLIRExpr::Lit { width: Width::Concrete(width), value })
        }
        // Wildcard / enum-variant selectors are handled via the case `default`
        // arm or enum encoding, not implemented for M1's scalar cases.
        SHIRPattern::Wildcard | SHIRPattern::EnumVariant { .. } => {
            Err(VLIRLowerError::TuplePatternUnsupported)
        }
    }
}

/// Flatten a (possibly nested) tuple pattern of literals into `(width, value)`,
/// first element most-significant. A wildcard *inside* a tuple has no single
/// selector value, so it is rejected rather than silently mis-matched.
fn flatten_tuple_pattern(p: &copper_core::shir::SHIRPattern) -> LowerResult<(usize, u128)> {
    use copper_core::shir::SHIRPattern;
    match p {
        SHIRPattern::Lit(lit) => Ok((width_of(&lit.ty).concrete(), lit.value)),
        SHIRPattern::Tuple(elems) => {
            let mut width = 0usize;
            let mut value = 0u128;
            for e in elems {
                let (w, v) = flatten_tuple_pattern(e)?;
                value = (value << w) | (v & mask_for(w));
                width += w;
            }
            Ok((width, value))
        }
        // A const name has no compile-time value here, so it cannot join a
        // concatenated tuple selector.
        SHIRPattern::Wildcard | SHIRPattern::EnumVariant { .. } | SHIRPattern::Const(_) => {
            Err(VLIRLowerError::TuplePatternUnsupported)
        }
    }
}

fn mask_for(width: usize) -> u128 {
    if width >= 128 { u128::MAX } else { (1u128 << width) - 1 }
}

// ── Latch checking ──────────────────────────────────────────────────────────

/// Signals assigned on *every* path through `stmts`.
fn assigned_on_all_paths(stmts: &[VLIRStmt]) -> HashSet<String> {
    let mut all = HashSet::new();
    for s in stmts {
        match s {
            VLIRStmt::WireAssign { name, .. } => {
                all.insert(name.clone());
            }
            VLIRStmt::PortAssign { port_name, .. } => {
                all.insert(port_name.clone());
            }
            // Without an `else`, the then-branch may be skipped entirely.
            VLIRStmt::If { then_stmts, else_stmts: Some(e), .. } => {
                let both: HashSet<String> = assigned_on_all_paths(then_stmts)
                    .intersection(&assigned_on_all_paths(e))
                    .cloned()
                    .collect();
                all.extend(both);
            }
            VLIRStmt::If { else_stmts: None, .. } => {}
            VLIRStmt::Case { arms, default, .. } => {
                // A `case` covers every path if it has a `default`, or if its arm
                // labels enumerate all 2^width selector values (e.g. four arms on
                // a 2-bit enum selector) — otherwise some value hits no arm.
                let complete = default.is_some() || case_is_exhaustive(arms);
                if complete {
                    let mut common: Option<HashSet<String>> =
                        default.as_ref().map(|d| assigned_on_all_paths(d));
                    for a in arms {
                        let s = assigned_on_all_paths(&a.stmts);
                        common = Some(match common {
                            None => s,
                            Some(c) => c.intersection(&s).cloned().collect(),
                        });
                    }
                    all.extend(common.unwrap_or_default());
                }
            }
            // A `for` loop may iterate zero times (bound could be 0), so nothing
            // it assigns is guaranteed on all paths — conservative, and correct.
            VLIRStmt::ForLoop { .. } => {}
            // A single-bit assign is a partial drive of `base`; not all-paths.
            VLIRStmt::IndexAssign { .. } => {}
        }
    }
    all
}

/// True when a `default`-less `case`'s arm labels enumerate every value the
/// selector can take (all `2^width` of them), making it complete.
fn case_is_exhaustive(arms: &[VLIRCaseArm]) -> bool {
    let mut values = HashSet::new();
    let mut width: Option<usize> = None;
    for a in arms {
        match &a.selector_value {
            VLIRExpr::Lit { width: w, value } => {
                let wc = w.concrete();
                if *width.get_or_insert(wc) != wc {
                    return false; // inconsistent label widths — be conservative
                }
                values.insert(*value);
            }
            _ => return false, // non-literal label: cannot reason about coverage
        }
    }
    match width {
        Some(w) if w < 64 => values.len() as u128 == 1u128 << w,
        _ => false,
    }
}

// ── Conditional-output → implicit-hold register (conditional/phased-output semantics) ──

/// Output ports driven (via `PortAssign`) on *every* path through `stmts`.
/// Mirrors `assigned_on_all_paths` but counts only output-port drives.
fn ports_driven_all_paths(stmts: &[VLIRStmt]) -> HashSet<String> {
    let mut all = HashSet::new();
    for s in stmts {
        match s {
            VLIRStmt::PortAssign { port_name, .. } => { all.insert(port_name.clone()); }
            VLIRStmt::If { then_stmts, else_stmts: Some(e), .. } => {
                let both: HashSet<String> = ports_driven_all_paths(then_stmts)
                    .intersection(&ports_driven_all_paths(e)).cloned().collect();
                all.extend(both);
            }
            VLIRStmt::If { else_stmts: None, .. } => {}
            VLIRStmt::Case { arms, default, .. } => {
                if default.is_some() || case_is_exhaustive(arms) {
                    let mut common: Option<HashSet<String>> = default.as_ref().map(|d| ports_driven_all_paths(d));
                    for a in arms {
                        let s = ports_driven_all_paths(&a.stmts);
                        common = Some(match common { None => s, Some(c) => c.intersection(&s).cloned().collect() });
                    }
                    all.extend(common.unwrap_or_default());
                }
            }
            _ => {}
        }
    }
    all
}

/// Output ports driven on *at least one* path through `stmts`.
fn ports_driven_any_path(stmts: &[VLIRStmt]) -> HashSet<String> {
    let mut any = HashSet::new();
    for s in stmts {
        match s {
            VLIRStmt::PortAssign { port_name, .. } => { any.insert(port_name.clone()); }
            VLIRStmt::If { then_stmts, else_stmts, .. } => {
                any.extend(ports_driven_any_path(then_stmts));
                if let Some(e) = else_stmts { any.extend(ports_driven_any_path(e)); }
            }
            VLIRStmt::Case { arms, default, .. } => {
                for a in arms { any.extend(ports_driven_any_path(&a.stmts)); }
                if let Some(d) = default { any.extend(ports_driven_any_path(d)); }
            }
            VLIRStmt::ForLoop { body, .. } => any.extend(ports_driven_any_path(body)),
            _ => {}
        }
    }
    any
}

/// Names referenced by an SHIR expression.
pub(crate) fn shir_expr_vars(e: &SHIRExpr, out: &mut HashSet<String>) {
    match e {
        SHIRExpr::Var(n) => {
            out.insert(n.clone());
        }
        SHIRExpr::MemIndex { mem, addr } => {
            out.insert(mem.clone());
            shir_expr_vars(addr, out);
        }
        SHIRExpr::BinOp { left, right, .. } => {
            shir_expr_vars(left, out);
            shir_expr_vars(right, out);
        }
        SHIRExpr::UnOp { expr, .. }
        | SHIRExpr::Slice { expr, .. }
        | SHIRExpr::Resize { expr, .. }
        | SHIRExpr::SignCast { expr, .. } => {
            shir_expr_vars(expr, out)
        }
        SHIRExpr::Mux { cond, then_val, else_val } => {
            shir_expr_vars(cond, out);
            shir_expr_vars(then_val, out);
            shir_expr_vars(else_val, out);
        }
        SHIRExpr::Case { scrutinee, arms, default } => {
            shir_expr_vars(scrutinee, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    shir_expr_vars(g, out);
                }
                shir_expr_vars(&a.value, out);
            }
            shir_expr_vars(default, out);
        }
        SHIRExpr::Concat(parts) => {
            for p in parts {
                shir_expr_vars(p, out);
            }
        }
        SHIRExpr::DynBit { base, index } => {
            shir_expr_vars(base, out);
            shir_expr_vars(index, out);
        }
        // A memory read result names nets synthesized by the memory lowering,
        // not signals the phase analysis can move or promote. A multi-phase module
        // may not drive a plain `Out` from one at all (see
        // `reject_memory_driven_comb_outputs`), so the phase-hold analysis never
        // has to reason about one.
        SHIRExpr::Lit(_)
        | SHIRExpr::PhaseEq(_)
        | SHIRExpr::MemData { .. }
        | SHIRExpr::MemValid { .. } => {}
    }
}

/// Combinational wire names declared in a phase's pre-edge statements.
fn shir_phase_local_wires(stmts: &[SHIRStmt], out: &mut HashSet<String>) {
    for s in stmts {
        match s {
            SHIRStmt::Wire { name, .. } => {
                out.insert(name.clone());
            }
            SHIRStmt::If { then_stmts, else_stmts, .. } => {
                shir_phase_local_wires(then_stmts, out);
                if let Some(e) = else_stmts {
                    shir_phase_local_wires(e, out);
                }
            }
            SHIRStmt::Match { arms, .. } => {
                for a in arms {
                    shir_phase_local_wires(&a.stmts, out);
                }
            }
            SHIRStmt::ForLoop { body, .. } => shir_phase_local_wires(body, out),
            SHIRStmt::PortDrive { .. }
            | SHIRStmt::IndexAssign { .. }
            | SHIRStmt::MemRead { .. }
            | SHIRStmt::MemWrite { .. } => {}
        }
    }
}

/// Output-port drives in a phase, as (port, names the driven value reads).
fn shir_port_drives(stmts: &[SHIRStmt], out: &mut Vec<(String, HashSet<String>)>) {
    for s in stmts {
        match s {
            SHIRStmt::PortDrive { port_name, value, .. } => {
                let mut vars = HashSet::new();
                shir_expr_vars(value, &mut vars);
                out.push((port_name.clone(), vars));
            }
            SHIRStmt::If { then_stmts, else_stmts, .. } => {
                shir_port_drives(then_stmts, out);
                if let Some(e) = else_stmts {
                    shir_port_drives(e, out);
                }
            }
            SHIRStmt::Match { arms, .. } => {
                for a in arms {
                    shir_port_drives(&a.stmts, out);
                }
            }
            SHIRStmt::ForLoop { body, .. } => shir_port_drives(body, out),
            SHIRStmt::Wire { .. }
            | SHIRStmt::IndexAssign { .. }
            | SHIRStmt::MemRead { .. }
            | SHIRStmt::MemWrite { .. } => {}
        }
    }
}

/// Output ports that must HOLD across the phases that do not write them, because
/// the value they are driven from does **not** survive outside its own phase.
///
/// A top-level drive lowers to a module-level continuous `assign`, which has no
/// phase guard and therefore reads its right-hand side on *every* cycle. Whether
/// that is correct depends entirely on what the right-hand side is outside the
/// writing phase:
///
/// * a **register** (or port, or constant) retains its value, so the continuous
///   assign is right — `mac_pipeline`'s `out.write(sum)` reads the inferred
///   register `sum_r`, and `assign out = sum_r` tracks it correctly;
/// * a **phase-local combinational wire** is defaulted to `'0` at the top of
///   `always_comb`, so the continuous assign propagates zeros — `sipo_block`'s
///   `w0_dbg.write(w0)` reads the phase-0 wire `w0`, and `assign w0_dbg = w0`
///   collapses to 0 on every other cycle.
///
/// Only the second case needs converting to an implicit-hold register, which is
/// the simulator's semantics: a sequential plain `Out` holds when unwritten (the
/// enabled-register idiom, verified against BaseJump's `bsg_dff_en`).
///
/// Narrow by construction: a port written in EVERY phase is driven on every cycle
/// and is excluded, and so is a port whose value is register-backed. Getting this
/// wrong in the widening direction lags an output by a cycle — `mac_pipeline` is
/// the measured witness, which is why the rule keys on the value and not merely on
/// "written in some phases but not all".
/// Reject a plain `Out` driven directly from a memory read result in a
/// **multi-phase** module.
///
/// Neither available form is right, which is why this is an error rather than a
/// choice. A module-level continuous assign tracks the read pipeline, which
/// re-captures on every edge, so the output would change again in the phases that
/// do not observe it. Converting it to the usual implicit-hold register latches it
/// at the end of the observing phase, one edge after the capture the simulator
/// reads — measured: `data` came out a full cycle late on every sampled value.
///
/// `RegOut` is the form that works, and it is the same `Out`/`RegOut` distinction
/// the pre-tick alignment guardrail points at. A single-phase module is unaffected
/// — there the post-tick segment shares the phase, and a plain `Out` driven from
/// `data()` is correct and covered by `tests/multiphase_memory_equivalence.rs`.
fn reject_memory_driven_comb_outputs(
    phases: &[SHIRPhase],
    registered_outs: &HashSet<String>,
    leg: &Legalizer,
) -> LowerResult<()> {
    if phases.len() < 2 {
        return Ok(());
    }
    for phase in phases {
        let mut drives = Vec::new();
        shir_port_drives_with_values(&phase.pre_edge, &mut drives);
        for (port, value) in drives {
            if registered_outs.contains(&leg.get(&port)) {
                continue;
            }
            let (mut data, mut valid) = (HashSet::new(), HashSet::new());
            collect_read_uses_expr(value, &mut data, &mut valid);
            if !data.is_empty() || !valid.is_empty() {
                return Err(VLIRLowerError::MemoryResultDrivesPlainOutput { port });
            }
        }
    }
    Ok(())
}

/// Output-port drives paired with the expression driving them.
fn shir_port_drives_with_values<'a>(
    stmts: &'a [SHIRStmt],
    out: &mut Vec<(String, &'a SHIRExpr)>,
) {
    for s in stmts {
        match s {
            SHIRStmt::PortDrive { port_name, value, .. } => out.push((port_name.clone(), value)),
            SHIRStmt::If { then_stmts, else_stmts, .. } => {
                shir_port_drives_with_values(then_stmts, out);
                if let Some(e) = else_stmts {
                    shir_port_drives_with_values(e, out);
                }
            }
            SHIRStmt::Match { arms, .. } => {
                for a in arms {
                    shir_port_drives_with_values(&a.stmts, out);
                }
            }
            SHIRStmt::ForLoop { body, .. } => shir_port_drives_with_values(body, out),
            SHIRStmt::Wire { .. }
            | SHIRStmt::IndexAssign { .. }
            | SHIRStmt::MemRead { .. }
            | SHIRStmt::MemWrite { .. } => {}
        }
    }
}

fn phase_scoped_output_ports(phases: &[SHIRPhase]) -> HashSet<String> {
    if phases.len() < 2 {
        return HashSet::new();
    }

    let mut drives_per_phase: Vec<Vec<(String, HashSet<String>)>> = Vec::new();
    let mut locals_per_phase: Vec<HashSet<String>> = Vec::new();
    for p in phases {
        let mut d = Vec::new();
        shir_port_drives(&p.pre_edge, &mut d);
        drives_per_phase.push(d);
        let mut w = HashSet::new();
        shir_phase_local_wires(&p.pre_edge, &mut w);
        locals_per_phase.push(w);
    }

    let written_in: Vec<HashSet<String>> = drives_per_phase
        .iter()
        .map(|d| d.iter().map(|(port, _)| port.clone()).collect())
        .collect();

    let mut hold = HashSet::new();
    for (idx, drives) in drives_per_phase.iter().enumerate() {
        for (port, reads) in drives {
            // Driven on every cycle → genuinely combinational.
            if written_in.iter().all(|w| w.contains(port)) {
                continue;
            }
            // Register-backed value → the continuous assign already holds.
            if reads.is_disjoint(&locals_per_phase[idx]) {
                continue;
            }
            hold.insert(port.clone());
        }
    }
    hold
}

/// Output ports driven on some-but-not-all paths — a conditional output./// Output ports driven on some-but-not-all paths — a conditional output. In a
/// sequential module these become **implicit-hold registers**: the `.write()`
/// holds its value between writes, so the drive belongs in `always_ff`
/// (`if (guard) out <= v`, holding otherwise) rather than `always_comb` (where an
/// undriven path is a latch). This is the sim's semantics for a conditionally
/// written output.
fn conditional_output_ports(stmts: &[VLIRStmt]) -> HashSet<String> {
    let all = ports_driven_all_paths(stmts);
    ports_driven_any_path(stmts).difference(&all).cloned().collect()
}

/// Give a Moore output decoded from a state `case` a defined default at the TOP of
/// `always_comb` (`out = <first-arm value>; case(state) … endcase`), so it reads as
/// combinational instead of being wrongly registered.
///
/// A `case` over a state register whose encoding leaves unreachable values (e.g. 6
/// states in a 3-bit register, or a `u8` program counter with two states) is not
/// exhaustive, so [`ports_driven_all_paths`] refuses to count *any* output as fully
/// driven — even one written on every explicit arm — and the caller then moves it
/// to `always_ff`, lagging it one cycle. A top-of-block default assignment closes
/// that unreachable path: the output is now driven on all paths and stays
/// combinational, matching the simulator's same-cycle Moore semantics.
///
/// Deliberately narrow, to preserve the two behaviours that must NOT change:
///   * only ports that are *currently conditional* are touched — a design like
///     `traffic_light` (exhaustive `case`, nothing conditional) is left byte-for-byte
///     identical;
///   * only ports driven on *every explicit arm* are hoisted — a genuine
///     enabled-register hold (`bsg_dff_en`, driven in only some arms) is not, and
///     still lands in `always_ff`.
///
/// The default value is cloned from the first arm that drives the port with a direct
/// assignment; its value is irrelevant (those `case` values are unreachable), it
/// only needs to be well-typed.
fn hoist_moore_output_defaults(stmts: &mut Vec<VLIRStmt>) {
    let conditional = conditional_output_ports(stmts);
    if conditional.is_empty() {
        return;
    }

    // First pass (immutable): decide which ports to hoist, and which no-default
    // cases we hoisted from (they need an empty `default` for Verilator).
    let mut to_hoist: Vec<(String, VLIRExpr)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut cases_needing_default: Vec<usize> = Vec::new();
    for (idx, s) in stmts.iter().enumerate() {
        let VLIRStmt::Case { arms, default, .. } = s else { continue };
        if arms.is_empty() {
            continue;
        }
        // Ports driven on all paths of *every* explicit arm.
        let mut common: Option<HashSet<String>> = None;
        for a in arms {
            let driven = ports_driven_all_paths(&a.stmts);
            common = Some(match common {
                None => driven,
                Some(c) => c.intersection(&driven).cloned().collect(),
            });
        }
        // An explicit `default` arm is a REACHABLE path, and one that drives
        // nothing means the output HOLDS — the enabled-register idiom, which this
        // pass exists not to disturb. Only the arms were being intersected, so
        // `match s { One => o.write(..), _ => {} }` looked fully driven and got an
        // unconditional default hoisted over it, turning a hold into a write every
        // cycle. `default: None` is the case this pass is FOR: no source
        // fall-through at all, only encodings the state register cannot reach.
        let mut common = common.unwrap_or_default();
        if let Some(d) = default {
            common = common.intersection(&ports_driven_all_paths(d)).cloned().collect();
        }
        let mut ports: Vec<&String> = common.intersection(&conditional).collect();
        ports.sort(); // deterministic emission order
        let mut hoisted_here = false;
        for port in ports {
            if !seen.insert(port.clone()) {
                continue;
            }
            if let Some(value) = first_direct_port_value(arms, port) {
                to_hoist.push((port.clone(), value));
                hoisted_here = true;
            }
        }
        if hoisted_here && default.is_none() {
            cases_needing_default.push(idx);
        }
    }

    if to_hoist.is_empty() {
        return;
    }

    // The top default makes the port combinationally driven, but a `case` over a
    // register with unreachable encodings is still incomplete — Verilator errors on
    // CASEINCOMPLETE. Give those cases an empty `default` (the fall-through value is
    // the hoisted top default). Mutate before the inserts below so indices hold.
    for idx in cases_needing_default {
        if let VLIRStmt::Case { default, .. } = &mut stmts[idx] {
            if default.is_none() {
                *default = Some(Vec::new());
            }
        }
    }

    // Prepend the defaults (first-seen order) ahead of the case that overrides them.
    for (port_name, value) in to_hoist.into_iter().rev() {
        // A hoisted default exists to make the port driven on all paths so it stays
        // COMBINATIONAL; it is never split into `always_ff`, so both forms are the
        // same expression.
        stmts.insert(0, VLIRStmt::PortAssign { port_name, value: value.clone(), edge_value: value });
    }
}

/// The value assigned to `port` by the first arm that drives it with a direct,
/// top-level `PortAssign` (not a nested/conditional drive).
fn first_direct_port_value(arms: &[VLIRCaseArm], port: &str) -> Option<VLIRExpr> {
    for a in arms {
        for s in &a.stmts {
            if let VLIRStmt::PortAssign { port_name, value, .. } = s {
                if port_name == port {
                    return Some(value.clone());
                }
            }
        }
    }
    None
}

/// Split the target output ports' drives out of `stmts` (combinational) and into
/// mirrored `always_ff` non-blocking assigns, preserving the surrounding
/// `if`/`case` guard structure. Returns (combinational remainder, ff updates).
/// Drop `always_comb` wire assignments that nothing reads any more.
///
/// Sequential forwarding (`shir_lower::Forwarding`) inlines a `let` wire's definition
/// into any expression that is sampled at the clock edge — `TODO` cause L-2. When a
/// wire's ONLY reader was such an expression, the wire is left assigned and unread,
/// which the equivalence harness rejects: Verilator runs under `-Wall`, where that is
/// `UNUSEDSIGNAL` and fails the build. Removing the dead assignment is part of doing
/// the inlining, not cosmetic tidying.
///
/// # Why here and not in `shir_lower`
///
/// This is the first point at which liveness is EXACT. A `SHIRStmt::PortDrive` carries
/// both an edge-sampled and a continuous form, and only one of them survives — so at
/// SHIR level a wire read solely by the discarded form still looks live. It was tried
/// there first and did not remove the wire.
///
/// # Why this cannot delete something live
///
/// Every collector below matches its enum EXHAUSTIVELY, with no `_` arm. A variant
/// added later fails to compile rather than silently going uncounted, which is the
/// only failure mode that would matter — deleting a wire something still reads.
fn drop_unread_wires(
    comb_phases: &mut [VLIRCombPhase],
    always_ff: &VLIRAlwaysFF,
    output_assigns: &[VLIRContinuousAssign],
    memories: &[VLIRMemDecl],
    // Nets read OUTSIDE this module — a received memory's bus outputs.
    external_reads: &HashSet<String>,
) {
    loop {
        let mut read: HashSet<String> = external_reads.clone();
        for phase in comb_phases.iter() {
            if let Some(g) = &phase.phase_guard {
                collect_vlir_reads(g, &mut read);
            }
            for st in &phase.stmts {
                collect_vlir_reads_stmt(st, &mut read);
            }
        }
        for st in &always_ff.stmts {
            collect_vlir_reads_ff(st, &mut read);
        }
        for a in output_assigns {
            collect_vlir_reads(&a.value, &mut read);
        }
        // A memory's read-data nets are assigned from the array rather than by a
        // statement, and their value expressions index it with ordinary wires —
        // those addresses must stay live.
        for m in memories {
            for net in &m.read_data_nets {
                collect_vlir_reads(&net.value, &mut read);
            }
        }

        let mut removed = false;
        for phase in comb_phases.iter_mut() {
            // A wire read NOWHERE loses every assignment, at any depth — with
            // all of them gone the declaration disappears too, so no partial
            // conditional structure (and hence no latch) can result. That is
            // the difference from dropping a SINGLE nested arm of a wire that
            // is still read, which would create one; reads gate the whole
            // removal above.
            fn prune(stmts: &mut Vec<VLIRStmt>, read: &HashSet<String>) -> bool {
                let before = stmts.len();
                stmts.retain(|st| match st {
                    VLIRStmt::WireAssign { name, .. } => read.contains(name),
                    _ => true,
                });
                let mut removed = stmts.len() != before;
                for st in stmts.iter_mut() {
                    match st {
                        VLIRStmt::If { then_stmts, else_stmts, .. } => {
                            removed |= prune(then_stmts, read);
                            if let Some(e) = else_stmts {
                                removed |= prune(e, read);
                            }
                        }
                        VLIRStmt::Case { arms, default, .. } => {
                            for a in arms {
                                removed |= prune(&mut a.stmts, read);
                            }
                            if let Some(d) = default {
                                removed |= prune(d, read);
                            }
                        }
                        VLIRStmt::ForLoop { body, .. } => {
                            removed |= prune(body, read);
                        }
                        _ => {}
                    }
                }
                removed
            }
            removed |= prune(&mut phase.stmts, &read);
        }
        // Dropping one wire can orphan another, so iterate to a fixpoint.
        if !removed {
            return;
        }
    }
}

fn collect_vlir_reads_stmt(stmt: &VLIRStmt, out: &mut HashSet<String>) {
    match stmt {
        VLIRStmt::WireAssign { value, .. } => collect_vlir_reads(value, out),
        VLIRStmt::PortAssign { value, edge_value, .. } => {
            collect_vlir_reads(value, out);
            collect_vlir_reads(edge_value, out);
        }
        // BOTH forms of the test count as reads, for the same reason `PortAssign`
        // collects both: which one survives is decided later, at
        // `split_output_reg`, and a wire read only by the surviving form must not
        // have been deleted by then.
        VLIRStmt::If { condition, edge_condition, then_stmts, else_stmts } => {
            collect_vlir_reads(condition, out);
            collect_vlir_reads(edge_condition, out);
            for s in then_stmts {
                collect_vlir_reads_stmt(s, out);
            }
            if let Some(e) = else_stmts {
                for s in e {
                    collect_vlir_reads_stmt(s, out);
                }
            }
        }
        VLIRStmt::Case { selector, edge_selector, arms, default } => {
            collect_vlir_reads(selector, out);
            collect_vlir_reads(edge_selector, out);
            for a in arms {
                collect_vlir_reads(&a.selector_value, out);
                for s in &a.stmts {
                    collect_vlir_reads_stmt(s, out);
                }
            }
            if let Some(d) = default {
                for s in d {
                    collect_vlir_reads_stmt(s, out);
                }
            }
        }
        VLIRStmt::ForLoop { start, end, body, .. } => {
            collect_vlir_reads(start, out);
            collect_vlir_reads(end, out);
            for s in body {
                collect_vlir_reads_stmt(s, out);
            }
        }
        // `base` is read-modify-written.
        VLIRStmt::IndexAssign { base, index, value } => {
            out.insert(base.clone());
            collect_vlir_reads(index, out);
            collect_vlir_reads(value, out);
        }
    }
}

fn collect_vlir_reads_ff(stmt: &VLIRFFStmt, out: &mut HashSet<String>) {
    match stmt {
        VLIRFFStmt::NonBlockingAssign { value, .. } => collect_vlir_reads(value, out),
        VLIRFFStmt::MemAssign { addr, value, .. } => {
            collect_vlir_reads(addr, out);
            collect_vlir_reads(value, out);
        }
        VLIRFFStmt::If { condition, then_stmts, else_stmts } => {
            collect_vlir_reads(condition, out);
            for s in then_stmts {
                collect_vlir_reads_ff(s, out);
            }
            if let Some(e) = else_stmts {
                for s in e {
                    collect_vlir_reads_ff(s, out);
                }
            }
        }
        VLIRFFStmt::Case { selector, arms, default } => {
            collect_vlir_reads(selector, out);
            for a in arms {
                collect_vlir_reads(&a.selector_value, out);
                for s in &a.stmts {
                    collect_vlir_reads_ff(s, out);
                }
            }
            if let Some(d) = default {
                for s in d {
                    collect_vlir_reads_ff(s, out);
                }
            }
        }
    }
}

fn collect_vlir_reads(expr: &VLIRExpr, out: &mut HashSet<String>) {
    match expr {
        VLIRExpr::Var(name) => {
            out.insert(name.clone());
        }
        VLIRExpr::Lit { .. } => {}
        VLIRExpr::BinOp { left, right, .. } => {
            collect_vlir_reads(left, out);
            collect_vlir_reads(right, out);
        }
        VLIRExpr::UnOp { expr, .. } => collect_vlir_reads(expr, out),
        VLIRExpr::Ternary { cond, then_val, else_val } => {
            collect_vlir_reads(cond, out);
            collect_vlir_reads(then_val, out);
            collect_vlir_reads(else_val, out);
        }
        VLIRExpr::SignCast { expr, .. } => collect_vlir_reads(expr, out),
        VLIRExpr::Concat(parts) => {
            for p in parts {
                collect_vlir_reads(p, out);
            }
        }
        VLIRExpr::Slice { expr, .. } => collect_vlir_reads(expr, out),
        VLIRExpr::DynBit { base, index } => {
            collect_vlir_reads(base, out);
            collect_vlir_reads(index, out);
        }
        VLIRExpr::Resize { expr, .. } => collect_vlir_reads(expr, out),
        VLIRExpr::MemIndex { mem, addr } => {
            out.insert(mem.clone());
            collect_vlir_reads(addr, out);
        }
    }
}

fn split_output_regs(stmts: &[VLIRStmt], targets: &HashSet<String>) -> (Vec<VLIRStmt>, Vec<VLIRFFStmt>) {
    let mut comb = Vec::new();
    let mut ff = Vec::new();
    for s in stmts {
        let (c, mut f) = split_output_reg(s, targets);
        if let Some(c) = c { comb.push(c); }
        ff.append(&mut f);
    }
    (comb, ff)
}

fn split_output_reg(s: &VLIRStmt, targets: &HashSet<String>) -> (Option<VLIRStmt>, Vec<VLIRFFStmt>) {
    match s {
        // The one point where a drive actually becomes edge-sampled, and therefore
        // the one place the choice between the two forms can be made correctly —
        // `targets` is only known here, after `hoist_moore_output_defaults` has had
        // its say. See `SHIRStmt::PortDrive`.
        VLIRStmt::PortAssign { port_name, edge_value, .. } if targets.contains(port_name) => {
            (None, vec![VLIRFFStmt::NonBlockingAssign {
                target: port_name.clone(),
                value: edge_value.clone(),
            }])
        }
        // A branch is split the same way its drives are, and each half takes the
        // matching form of the test: the `always_comb` copy reads the registers
        // AFTER the edge (`condition`), the `always_ff` copy is sampled BEFORE it
        // (`edge_condition`). Using `condition` for both is `TODO` cause N.
        VLIRStmt::If { condition, edge_condition, then_stmts, else_stmts } => {
            let (tc, tf) = split_output_regs(then_stmts, targets);
            let (ec, ef) = match else_stmts {
                Some(e) => { let (c, f) = split_output_regs(e, targets); (Some(c), f) }
                None => (None, Vec::new()),
            };
            let comb = if tc.is_empty() && ec.as_ref().map_or(true, |c| c.is_empty()) {
                None
            } else {
                Some(VLIRStmt::If {
                    condition: condition.clone(),
                    edge_condition: edge_condition.clone(),
                    then_stmts: tc,
                    else_stmts: ec,
                })
            };
            let ff = if tf.is_empty() && ef.is_empty() {
                Vec::new()
            } else {
                vec![VLIRFFStmt::If {
                    condition: edge_condition.clone(),
                    then_stmts: tf,
                    else_stmts: if ef.is_empty() { None } else { Some(ef) },
                }]
            };
            (comb, ff)
        }
        // Same split, same choice of form — see the `If` arm.
        VLIRStmt::Case { selector, edge_selector, arms, default } => {
            let mut comb_arms = Vec::new();
            let mut ff_arms = Vec::new();
            for a in arms {
                let (ac, af) = split_output_regs(&a.stmts, targets);
                if !ac.is_empty() { comb_arms.push(VLIRCaseArm { selector_value: a.selector_value.clone(), stmts: ac }); }
                if !af.is_empty() { ff_arms.push(VLIRFFCaseArm { selector_value: a.selector_value.clone(), stmts: af }); }
            }
            let (comb_default, ff_default) = match default {
                Some(d) => {
                    let (c, f) = split_output_regs(d, targets);
                    (if c.is_empty() { None } else { Some(c) }, if f.is_empty() { None } else { Some(f) })
                }
                None => (None, None),
            };
            let comb = if comb_arms.is_empty() && comb_default.is_none() {
                None
            } else {
                Some(VLIRStmt::Case {
                    selector: selector.clone(),
                    edge_selector: edge_selector.clone(),
                    arms: comb_arms,
                    default: comb_default,
                })
            };
            let ff = if ff_arms.is_empty() && ff_default.is_none() {
                Vec::new()
            } else {
                // A registered hold needs a complete case: the empty default means
                // "no assignment → the output register holds" (and avoids a
                // Verilator CASEINCOMPLETE warning).
                vec![VLIRFFStmt::Case {
                    selector: edge_selector.clone(),
                    arms: ff_arms,
                    default: ff_default.or_else(|| Some(Vec::new())),
                }]
            };
            (comb, ff)
        }
        other => (Some(clone_comb_stmt(other)), Vec::new()),
    }
}

/// Signals assigned on *at least one* path through `stmts`.
fn assigned_on_any_path(stmts: &[VLIRStmt]) -> HashSet<String> {
    let mut any = HashSet::new();
    for s in stmts {
        match s {
            VLIRStmt::WireAssign { name, .. } => {
                any.insert(name.clone());
            }
            VLIRStmt::PortAssign { port_name, .. } => {
                any.insert(port_name.clone());
            }
            VLIRStmt::If { then_stmts, else_stmts, .. } => {
                any.extend(assigned_on_any_path(then_stmts));
                if let Some(e) = else_stmts {
                    any.extend(assigned_on_any_path(e));
                }
            }
            VLIRStmt::Case { arms, default, .. } => {
                for a in arms {
                    any.extend(assigned_on_any_path(&a.stmts));
                }
                if let Some(d) = default {
                    any.extend(assigned_on_any_path(d));
                }
            }
            VLIRStmt::ForLoop { body, .. } => {
                any.extend(assigned_on_any_path(body));
            }
            // A bit-assign drives `base` (partially) — it counts as touching it.
            VLIRStmt::IndexAssign { base, .. } => {
                any.insert(base.clone());
            }
        }
    }
    any
}

/// Reject any combinational block that would infer a latch — a signal assigned
/// Auto-hoist block-local combinational defaults.
///
/// A `let x = <default>` declared inside a conditional branch (e.g. a shift
/// temporary `let mut shifted = Bits::zero()` inside `if en { … }`) lowers to a
/// `WireAssign` nested in that branch. At module scope that leaves `x` assigned on
/// only some paths — a false latch, since `x` is only *read* on the path where it
/// is written. Rather than force the source to hoist the temp (a hardware concern
/// leaking into natural Rust), hoist the default here: move the literal-init
/// `WireAssign` to the top of the block so `x` is driven on every path. The
/// in-branch bit-assigns/reassigns remain and overwrite it as before.
///
/// Scoped narrowly: only a `WireAssign` with a *literal* value (a default) whose
/// name is not already assigned at the top level is hoisted — conditional drives
/// of real signals/ports are untouched.
/// Give branch-local temporaries an unconditional default, so a value that is
/// written and read entirely inside one branch does not read as a latch.
///
/// Two shapes, one rule. A `let` inside a branch is scoped to that branch in
/// Rust, so nothing outside can observe it and a default is always safe:
///
/// * a **literal** initializer is moved to the top outright — evaluating a
///   constant early is the same as evaluating it in the branch;
/// * a **computed** initializer gets a zero default at the top and keeps the
///   computation where it was written, because hoisting the computation itself
///   would evaluate it on paths the source never runs it on.
///
/// `protected` names are exempt: a register must never be given an
/// unconditional default, since a conditionally-driven register is the *hold*
/// idiom (`bsg_dff_en`), not a latch bug — defaulting it to zero would clear it
/// on every untaken path.
fn hoist_branch_local_defaults(stmts: &mut Vec<VLIRStmt>, protected: &HashSet<String>) {
    let top_assigned: HashSet<String> = stmts.iter().filter_map(top_level_assign_name).collect();
    let mut hoisted: Vec<VLIRStmt> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    strip_nested_defaults(stmts, false, &top_assigned, protected, &mut hoisted, &mut seen);
    // Prepend in original first-seen order.
    for h in hoisted.into_iter().rev() {
        stmts.insert(0, h);
    }
}

fn top_level_assign_name(s: &VLIRStmt) -> Option<String> {
    match s {
        VLIRStmt::WireAssign { name, .. } => Some(name.clone()),
        VLIRStmt::PortAssign { port_name, .. } => Some(port_name.clone()),
        // If/Case/ForLoop are conditional — they assign nothing unconditionally.
        _ => None,
    }
}

fn strip_nested_defaults(
    body: &mut Vec<VLIRStmt>,
    is_nested: bool,
    top_assigned: &HashSet<String>,
    protected: &HashSet<String>,
    hoisted: &mut Vec<VLIRStmt>,
    seen: &mut HashSet<String>,
) {
    let mut keep = Vec::with_capacity(body.len());
    for mut s in body.drain(..) {
        // Recurse into sub-bodies first (they are one level more nested).
        match &mut s {
            VLIRStmt::If { then_stmts, else_stmts, .. } => {
                strip_nested_defaults(then_stmts, true, top_assigned, protected, hoisted, seen);
                if let Some(e) = else_stmts {
                    strip_nested_defaults(e, true, top_assigned, protected, hoisted, seen);
                }
            }
            VLIRStmt::Case { arms, default, .. } => {
                for a in arms {
                    strip_nested_defaults(&mut a.stmts, true, top_assigned, protected, hoisted, seen);
                }
                if let Some(d) = default {
                    strip_nested_defaults(d, true, top_assigned, protected, hoisted, seen);
                }
            }
            VLIRStmt::ForLoop { body, .. } => {
                strip_nested_defaults(body, true, top_assigned, protected, hoisted, seen);
            }
            _ => {}
        }
        // A nested WireAssign to a name nothing assigns unconditionally is a
        // block-local temporary. A literal initializer moves to the top whole; a
        // computed one leaves a zero default behind and stays where it is.
        let hoistable = if is_nested {
            match &s {
                VLIRStmt::WireAssign { name, value, width, outer_dim }
                    if !top_assigned.contains(name) && !protected.contains(name) =>
                {
                    let default_width = if matches!(value, VLIRExpr::Lit { .. }) {
                        None
                    } else {
                        Some((width.clone(), outer_dim.clone()))
                    };
                    Some((name.clone(), default_width))
                }
                _ => None,
            }
        } else {
            None
        };
        match hoistable {
            // Literal: the FIRST assignment per name is the default and moves to
            // the top. A LATER literal assign to the same name is an OVERRIDE —
            // the default-then-override lowering writes exactly that shape
            // (`n_valid = 1'b1` inside a match arm, after the hoisted zero), and
            // two branch-scoped `let`s sharing a lowered name are the same
            // situation — so it STAYS where it was written. Stripping it too
            // silently deleted the write (found by the pipelined CPU's IF stage:
            // `if_id.valid = true` vanished and nothing ever decoded).
            Some((name, None)) => {
                if seen.insert(name) {
                    hoisted.push(s);
                    continue;
                }
                keep.push(s);
                continue;
            }
            // Computed: default at the top, computation stays in the branch.
            Some((name, Some((width, outer_dim)))) => {
                if seen.insert(name.clone()) {
                    hoisted.push(VLIRStmt::WireAssign {
                        name,
                        width: width.clone(),
                        outer_dim, // `'0` fills either shape
                        value: VLIRExpr::Lit { width, value: 0 },
                    });
                }
            }
            None => {}
        }
        keep.push(s);
    }
    *body = keep;
}

/// on some control paths but not all. Copper's premise is that this class of bug
/// should be impossible to express, so it is an error, not a silent emission.
fn check_no_latches(stmts: &[VLIRStmt]) -> LowerResult<()> {
    let mut latched: Vec<String> = assigned_on_any_path(stmts)
        .difference(&assigned_on_all_paths(stmts))
        .cloned()
        .collect();
    if latched.is_empty() {
        return Ok(());
    }
    latched.sort(); // deterministic diagnostics
    Err(VLIRLowerError::LatchInferred { signals: latched })
}

// ── Memory nets ─────────────────────────────────────────────────────────────
//
// A memory lowers to one packed array plus a small bundle of nets per accessed
// port. The address/enable/data *inputs* are ordinary `always_comb` wires, so a
// conditional access keeps its `if` structure and the enable falls out of it:
//
//     memory_wr0_en = 1'b0;                       // default, top of always_comb
//     if (ena && wea) begin
//         memory_wr0_en   = 1'b1;
//         memory_wr0_addr = addra;
//         memory_wr0_data = dia;
//     end
//     ...
//     assign memory_rd0_data = memory[memory_rd0_addr];   // continuous read
//     always_ff @(posedge clk)
//         if (memory_wr0_en) memory[memory_wr0_addr] <= memory_wr0_data;
//
// The read data net is a *continuous* read of the array, so a consumer that
// latches it at the edge (`if (rd_en) dob <= mem[addr]` after inlining) sees the
// pre-write contents — the ReadFirst ordering the simulator implements.

/// Address / element widths of each declared memory, keyed by its *source* name.
/// Threaded into statement lowering because a memory access has to size the nets
/// it drives and only the declaration knows those widths.
#[derive(Default)]
struct MemWidths {
    widths: HashMap<String, (Width, Width)>,
    /// Read ports that need an enable net (see `MemPortUse::needs_read_en`).
    read_en: HashSet<MemPort>,
}

impl MemWidths {
    fn build(memories: &[SHIRMemory], use_: &MemPortUse) -> Self {
        MemWidths {
            widths: memories
                .iter()
                .map(|m| {
                    let aw = if m.received {
                        // Depth unknown to the child — see `addr_param_name`.
                        Width::Param(addr_param_name(&m.name))
                    } else {
                        addr_width(m.depth)
                    };
                    (m.name.clone(), (aw, width_of(&m.elem_ty)))
                })
                .collect(),
            read_en: memories
                .iter()
                .flat_map(|m| (0..m.read_ports).map(|p| (m.name.clone(), p)))
                .filter(|k| use_.needs_read_en(k))
                .collect(),
        }
    }
    fn addr(&self, mem: &str) -> Width {
        self.widths.get(mem).map(|(a, _)| a.clone()).unwrap_or(Width::Concrete(1))
    }
    fn data(&self, mem: &str) -> Width {
        self.widths.get(mem).map(|(_, d)| d.clone()).unwrap_or(Width::Concrete(1))
    }
    fn wants_read_en(&self, mem: &str, port: usize) -> bool {
        self.read_en.contains(&(mem.to_string(), port))
    }
}

fn mem_net(mem: &str, is_read: bool, port: usize, suffix: &str) -> String {
    format!("{mem}_{}{port}_{suffix}", if is_read { "rd" } else { "wr" })
}

/// Read-pipeline stage `k` of a read port: `<m>_rd<i>_q<k>` (data) and
/// `<m>_rd<i>_v<k>` (valid). Stage `READ_LAT - 1` is the port's output; stage 0
/// holds what the most recent edge captured.
fn read_stage(mem: &str, port: usize, stage: usize, data: bool) -> String {
    mem_net(mem, true, port, &format!("{}{stage}", if data { "q" } else { "v" }))
}

/// Write-pipeline stage `k` of a write port (`k` in `1..WRITE_LAT`). Stage 0 is
/// the combinational `en`/`addr`/`data` nets — it is filled and consumed within
/// one cycle, so it needs no register. Stage `WRITE_LAT - 1` is what commits.
fn write_stage(mem: &str, port: usize, stage: usize, suffix: &str) -> String {
    mem_net(mem, false, port, &format!("s{stage}_{suffix}"))
}

/// The nets a write port's committing stage is held in: the combinational ones at
/// `WRITE_LAT == 1`, otherwise the last stage registers. `(en, addr, data)`.
fn write_commit_nets(mem: &str, port: usize, write_lat: usize) -> (String, String, String) {
    if write_lat == 1 {
        (
            mem_net(mem, false, port, "en"),
            mem_net(mem, false, port, "addr"),
            mem_net(mem, false, port, "data"),
        )
    } else {
        let k = write_lat - 1;
        (
            write_stage(mem, port, k, "v"),
            write_stage(mem, port, k, "addr"),
            write_stage(mem, port, k, "data"),
        )
    }
}

/// The module parameter carrying a RECEIVED memory's address width. The depth
/// of a received memory is a runtime constructor argument of the OWNER's
/// object — it is not in the type — so the child cannot size the address bus
/// concretely; a parameter (defaulting to 1, like every generic width) lets the
/// instantiating context supply it. Named from the source identifier, which is
/// a valid SystemVerilog identifier already.
fn addr_param_name(mem: &str) -> String {
    format!("{}_ADDR_W", mem.to_uppercase())
}

/// Bits needed to index `depth` entries (at least one).
fn addr_width(depth: usize) -> Width {
    let bits = if depth <= 1 { 1 } else { (usize::BITS - (depth - 1).leading_zeros()) as usize };
    Width::Concrete(bits)
}

/// A memory read port, identified by `(memory name, port index)`.
type MemPort = (String, usize);

/// Which nets a `MemData` / `MemValid` expression resolves to at one point in the
/// lowering. Built per (phase, region) by [`mem_binding`] — see the doc there for
/// why the answer is not the same everywhere.
type MemBinding = HashMap<MemPort, (String, String)>;

/// Which memory ports a body touches, and — for read results — in which FORM.
///
/// A read result has two forms, and picking the wrong one shifts the design by a
/// cycle in one direction or the other:
///
/// * the **combinational** net (`<m>_rd<i>_data`), a continuous read of the array.
///   Correct only for a consumer that latches at the SAME edge as the capture: an
///   `always_ff` register update in the phase that stages the read. There the
///   register and the memory capture the same pre-edge value together.
/// * the **registered** net (`<m>_rd<i>_q`), holding what the capture edge
///   produced. Correct for every other consumer — anything reading the result
///   *after* the capture edge, which includes a plain combinational use in the
///   same single-tick loop (it runs post-edge) and any use in a later phase.
#[derive(Default)]
struct MemPortUse {
    /// `(mem, port)` staged by a `read()` call.
    reads: HashSet<MemPort>,
    /// `(mem, port)` staged by a `write()` call.
    writes: HashSet<MemPort>,
    /// `data()` observed at the capture edge / after it.
    data_comb: HashSet<MemPort>,
    data_reg: HashSet<MemPort>,
    /// `is_ready()` observed at the capture edge / after it.
    valid_comb: HashSet<MemPort>,
    valid_reg: HashSet<MemPort>,
}

impl MemPortUse {
    /// A read port needs its enable/address nets if it is staged *or* merely
    /// observed: a `data()` read with no `read()` anywhere is a port that never
    /// becomes ready, which the emitted `en = 1'b0` default reproduces exactly.
    /// Without this the observation would reference nets nothing declares.
    fn read_touched(&self, k: &MemPort) -> bool {
        self.reads.contains(k)
            || self.data_comb.contains(k)
            || self.data_reg.contains(k)
            || self.valid_comb.contains(k)
            || self.valid_reg.contains(k)
    }
    /// The continuous array-read net is needed by a combinational consumer
    /// directly, and by a registered one as the register's input.
    fn needs_data_net(&self, k: &MemPort) -> bool {
        self.data_comb.contains(k) || self.data_reg.contains(k)
    }
    /// A READ port's enable exists only to answer `is_ready()` — the array read
    /// itself needs only an address. A design that never asks (a ROM read
    /// unconditionally, say) would otherwise carry a net nothing reads, which is a
    /// fatal Verilator UNUSEDSIGNAL under `-Wall`.
    fn needs_read_en(&self, k: &MemPort) -> bool {
        self.valid_comb.contains(k) || self.valid_reg.contains(k)
    }
}

/// The nets `MemData` / `MemValid` resolve to in one phase and region.
///
/// `staged_here` is the set of read ports this phase stages. A consumer in the
/// phase's **post_edge** (a register update) latches at the very edge that
/// captures, so it must read the combinational value; everything else runs after
/// that edge and must read the register.
fn mem_binding(
    memories: &[SHIRMemory],
    staged_here: &HashSet<MemPort>,
    post_edge: bool,
    leg: &Legalizer,
) -> MemBinding {
    let mut b = MemBinding::new();
    for m in memories {
        for port in 0..m.read_ports {
            let key = (m.name.clone(), port);
            let same_edge = post_edge && staged_here.contains(&key);
            let nets = if same_edge {
                // The value the port's OUTPUT will hold once this edge passes.
                // At one cycle of latency that is the live array read; deeper, it
                // is the stage about to shift into the output, read at its
                // pre-edge value inside `always_ff`.
                if m.read_lat == 1 {
                    (
                        leg.get(&mem_net(&m.name, true, port, "data")),
                        leg.get(&mem_net(&m.name, true, port, "en")),
                    )
                } else {
                    (
                        leg.get(&read_stage(&m.name, port, m.read_lat - 2, true)),
                        leg.get(&read_stage(&m.name, port, m.read_lat - 2, false)),
                    )
                }
            } else {
                // The port's output as it stands now.
                (
                    leg.get(&read_stage(&m.name, port, m.read_lat - 1, true)),
                    leg.get(&read_stage(&m.name, port, m.read_lat - 1, false)),
                )
            };
            b.insert(key, nets);
        }
    }
    b
}

/// Read ports staged by a `read()` somewhere in these statements.
fn staged_read_ports(stmts: &[SHIRStmt]) -> HashSet<MemPort> {
    let mut u = MemPortUse::default();
    collect_mem_use(stmts, &mut u);
    u.reads
}

/// Tally every port a body stages, and classify every read-result observation as
/// combinational or registered, phase by phase.
fn collect_phase_mem_use(phases: &[SHIRPhase], use_: &mut MemPortUse) {
    for phase in phases {
        collect_mem_use(&phase.pre_edge, use_);
        let staged = staged_read_ports(&phase.pre_edge);

        // Pre-edge statements run AFTER the edge that captured (in a single-tick
        // loop the post-tick segment lands here), so they always read the register.
        let (mut data, mut valid) = (HashSet::new(), HashSet::new());
        collect_read_uses(&phase.pre_edge, &mut data, &mut valid);
        use_.data_reg.extend(data.drain());
        use_.valid_reg.extend(valid.drain());

        // Register updates latch at this phase's edge: combinational iff this
        // phase is the one staging the read.
        for u in &phase.post_edge {
            collect_read_uses_expr(&u.next_value, &mut data, &mut valid);
        }
        for k in data {
            if staged.contains(&k) {
                use_.data_comb.insert(k);
            } else {
                use_.data_reg.insert(k);
            }
        }
        for k in valid {
            if staged.contains(&k) {
                use_.valid_comb.insert(k);
            } else {
                use_.valid_reg.insert(k);
            }
        }
    }
}

/// Staged accesses (`read()` / `write()`) in a statement tree.
fn collect_mem_use(stmts: &[SHIRStmt], use_: &mut MemPortUse) {
    for s in stmts {
        match s {
            SHIRStmt::MemRead { mem, port, .. } => {
                use_.reads.insert((mem.clone(), *port));
            }
            SHIRStmt::MemWrite { mem, port, .. } => {
                use_.writes.insert((mem.clone(), *port));
            }
            SHIRStmt::If { then_stmts, else_stmts, .. } => {
                collect_mem_use(then_stmts, use_);
                if let Some(e) = else_stmts {
                    collect_mem_use(e, use_);
                }
            }
            SHIRStmt::Match { arms, .. } => {
                for a in arms {
                    collect_mem_use(&a.stmts, use_);
                }
            }
            SHIRStmt::ForLoop { body, .. } => collect_mem_use(body, use_),
            SHIRStmt::Wire { .. } | SHIRStmt::PortDrive { .. } | SHIRStmt::IndexAssign { .. } => {}
        }
    }
}

/// Read-result observations (`data()` / `is_ready()`) in a statement tree.
fn collect_read_uses(
    stmts: &[SHIRStmt],
    data: &mut HashSet<MemPort>,
    valid: &mut HashSet<MemPort>,
) {
    for s in stmts {
        match s {
            SHIRStmt::Wire { value, .. } | SHIRStmt::PortDrive { value, .. } => {
                collect_read_uses_expr(value, data, valid)
            }
            SHIRStmt::MemRead { addr, .. } => collect_read_uses_expr(addr, data, valid),
            SHIRStmt::MemWrite { addr, value, .. } => {
                collect_read_uses_expr(addr, data, valid);
                collect_read_uses_expr(value, data, valid);
            }
            // Both forms of the test, since either may be the one emitted — the
            // same reason `collect_vlir_reads_stmt` walks both.
            SHIRStmt::If { condition, edge_condition, then_stmts, else_stmts } => {
                collect_read_uses_expr(condition, data, valid);
                collect_read_uses_expr(edge_condition, data, valid);
                collect_read_uses(then_stmts, data, valid);
                if let Some(e) = else_stmts {
                    collect_read_uses(e, data, valid);
                }
            }
            SHIRStmt::Match { scrutinee, edge_scrutinee, arms } => {
                collect_read_uses_expr(scrutinee, data, valid);
                collect_read_uses_expr(edge_scrutinee, data, valid);
                for a in arms {
                    if let Some(g) = &a.guard {
                        collect_read_uses_expr(g, data, valid);
                    }
                    collect_read_uses(&a.stmts, data, valid);
                }
            }
            SHIRStmt::ForLoop { start, end, body, .. } => {
                collect_read_uses_expr(start, data, valid);
                collect_read_uses_expr(end, data, valid);
                collect_read_uses(body, data, valid);
            }
            SHIRStmt::IndexAssign { index, value, .. } => {
                collect_read_uses_expr(index, data, valid);
                collect_read_uses_expr(value, data, valid);
            }
        }
    }
}

fn collect_read_uses_expr(
    e: &SHIRExpr,
    data: &mut HashSet<MemPort>,
    valid: &mut HashSet<MemPort>,
) {
    match e {
        SHIRExpr::MemIndex { addr, .. } => {
            collect_read_uses_expr(addr, data, valid);
        }
        SHIRExpr::MemData { mem, port } => {
            data.insert((mem.clone(), *port));
        }
        SHIRExpr::MemValid { mem, port } => {
            valid.insert((mem.clone(), *port));
        }
        SHIRExpr::Var(_) | SHIRExpr::Lit(_) | SHIRExpr::PhaseEq(_) => {}
        SHIRExpr::BinOp { left, right, .. } => {
            collect_read_uses_expr(left, data, valid);
            collect_read_uses_expr(right, data, valid);
        }
        SHIRExpr::UnOp { expr, .. }
        | SHIRExpr::Resize { expr, .. }
        | SHIRExpr::Slice { expr, .. }
        | SHIRExpr::SignCast { expr, .. } => collect_read_uses_expr(expr, data, valid),
        SHIRExpr::Mux { cond, then_val, else_val } => {
            collect_read_uses_expr(cond, data, valid);
            collect_read_uses_expr(then_val, data, valid);
            collect_read_uses_expr(else_val, data, valid);
        }
        SHIRExpr::Case { scrutinee, arms, default } => {
            collect_read_uses_expr(scrutinee, data, valid);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_read_uses_expr(g, data, valid);
                }
                collect_read_uses_expr(&a.value, data, valid);
            }
            collect_read_uses_expr(default, data, valid);
        }
        SHIRExpr::Concat(parts) => {
            for p in parts {
                collect_read_uses_expr(p, data, valid);
            }
        }
        SHIRExpr::DynBit { base, index } => {
            collect_read_uses_expr(base, data, valid);
            collect_read_uses_expr(index, data, valid);
        }
    }
}

/// Top-of-`always_comb` defaults for the memory nets THIS phase drives. Without
/// them an enable driven only inside an `if` would infer a latch (and
/// `check_no_latches` would reject the module). Scoped per phase because in a
/// multi-phase module each phase drives only its own accesses — the nets it does
/// not touch are cleared by the emitter's top-of-block defaults instead, which is
/// what makes an enable read as 0 outside its staging phase.
fn mem_net_defaults(
    memories: &[SHIRMemory],
    driven: &MemPortUse,
    all: &MemPortUse,
    leg: &Legalizer,
) -> Vec<VLIRStmt> {
    let mut out = Vec::new();
    let one_bit = Width::Concrete(1);
    for m in memories {
        let aw = if m.received {
            // The child cannot size a received memory's address bus concretely —
            // see `addr_param_name`; the default renders as `M_ADDR_W'd0`.
            Width::Param(addr_param_name(&m.name))
        } else {
            addr_width(m.depth)
        };
        let dw = width_of(&m.elem_ty);
        let mut push = |is_read: bool, port: usize, with_data: bool| {
            let en = (!is_read || all.needs_read_en(&(m.name.clone(), port)))
                .then(|| ("en", one_bit.clone()));
            for (suffix, width) in en
                .into_iter()
                .chain([("addr", aw.clone())])
                .chain(with_data.then(|| ("data", dw.clone())))
            {
                out.push(VLIRStmt::WireAssign {
                    name: leg.get(&mem_net(&m.name, is_read, port, suffix)),
                    width: width.clone(),
                    outer_dim: None, // a memory control net is never an array
                    value: VLIRExpr::Lit { width, value: 0 },
                });
            }
        };
        for port in 0..m.read_ports {
            if driven.reads.contains(&(m.name.clone(), port)) {
                push(true, port, false);
            }
        }
        for port in 0..m.write_ports {
            if driven.writes.contains(&(m.name.clone(), port)) {
                push(false, port, true);
            }
        }
    }
    out
}

/// Defaults for read ports that are OBSERVED but never staged anywhere. Their
/// enable/address nets are still referenced (by the observation, and by the
/// continuous array read), so they need one unconditional driver. A port that is
/// never staged never becomes ready, which `en = 1'b0` reproduces exactly.
fn unstaged_read_defaults(
    memories: &[SHIRMemory],
    use_: &MemPortUse,
    leg: &Legalizer,
) -> Vec<VLIRStmt> {
    let mut out = Vec::new();
    let one_bit = Width::Concrete(1);
    for m in memories {
        for port in 0..m.read_ports {
            let key = (m.name.clone(), port);
            if use_.reads.contains(&key) || !use_.read_touched(&key) {
                continue;
            }
            let en = use_
                .needs_read_en(&key)
                .then(|| ("en", one_bit.clone()));
            for (suffix, width) in en.into_iter().chain([("addr", addr_width(m.depth))]) {
                out.push(VLIRStmt::WireAssign {
                    name: leg.get(&mem_net(&m.name, true, port, suffix)),
                    width: width.clone(),
                    outer_dim: None,
                    value: VLIRExpr::Lit { width, value: 0 },
                });
            }
        }
    }
    out
}

/// How deep a read port's data / valid register chains have to be.
///
/// Stage `READ_LAT - 1` is the port's output, and a consumer that latches at the
/// capture edge reads stage `READ_LAT - 2` instead (see `mem_binding`), so the
/// deepest stage actually referenced depends on which forms are used. Returns
/// `None` when no register is needed at all — the one-cycle port whose only
/// consumer latches at the capture edge, which reads the array directly.
fn read_chain_depth(m: &SHIRMemory, port: usize, use_: &MemPortUse, data: bool) -> Option<usize> {
    let key = (m.name.clone(), port);
    let (delayed, same_edge) = if data {
        (use_.data_reg.contains(&key), use_.data_comb.contains(&key))
    } else {
        (use_.valid_reg.contains(&key), use_.valid_comb.contains(&key))
    };
    let mut deepest = None;
    if delayed {
        deepest = Some(m.read_lat - 1);
    }
    if same_edge && m.read_lat >= 2 {
        deepest = Some(deepest.map_or(m.read_lat - 2, |d: usize| d.max(m.read_lat - 2)));
    }
    deepest
}

/// The read pipeline: `q0 <= <array read>` and `qk <= q(k-1)`, plus the matching
/// valid chain.
///
/// Deliberately UNGUARDED by phase, because the simulator's pipeline advances on
/// every posedge regardless of what the design is doing (`advance_read_pipelines`
/// is a clock listener; the memory knows nothing about phases). Outside its
/// staging phase the enable net is 0, so the valid chain drains — which is exactly
/// the simulator's `is_ready()` after a cycle with no staged address.
///
/// The shift is written `qk <= q(k-1)` *and* `q0 <= data` in one block: with
/// non-blocking assignment every right-hand side reads the pre-edge value, which
/// is the same "shift, then capture into stage 0" the simulator performs.
fn mem_read_pipeline(
    memories: &[SHIRMemory],
    use_: &MemPortUse,
    leg: &Legalizer,
) -> Vec<VLIRFFStmt> {
    let mut out = Vec::new();
    for m in memories {
        for port in 0..m.read_ports {
            for (is_data, source) in [
                (true, mem_net(&m.name, true, port, "data")),
                (false, mem_net(&m.name, true, port, "en")),
            ] {
                let Some(deepest) = read_chain_depth(m, port, use_, is_data) else { continue };
                for stage in 0..=deepest {
                    let value = if stage == 0 {
                        VLIRExpr::Var(leg.get(&source))
                    } else {
                        VLIRExpr::Var(leg.get(&read_stage(&m.name, port, stage - 1, is_data)))
                    };
                    out.push(VLIRFFStmt::NonBlockingAssign {
                        target: leg.get(&read_stage(&m.name, port, stage, is_data)),
                        value,
                    });
                }
            }
        }
    }
    out
}

/// The write pipeline: `s1 <= <comb nets>` and `sk <= s(k-1)`, for a write port
/// whose commit is more than one cycle out. Stage 0 needs no register — it is
/// filled by the `write()` call and consumed in the same cycle.
fn mem_write_pipeline(
    memories: &[SHIRMemory],
    use_: &MemPortUse,
    leg: &Legalizer,
) -> Vec<VLIRFFStmt> {
    let mut out = Vec::new();
    for m in memories {
        for port in 0..m.write_ports {
            if !use_.writes.contains(&(m.name.clone(), port)) {
                continue;
            }
            for stage in 1..m.write_lat {
                for suffix in ["v", "addr", "data"] {
                    let value = if stage == 1 {
                        let comb = if suffix == "v" { "en" } else { suffix };
                        VLIRExpr::Var(leg.get(&mem_net(&m.name, false, port, comb)))
                    } else {
                        VLIRExpr::Var(leg.get(&write_stage(&m.name, port, stage - 1, suffix)))
                    };
                    out.push(VLIRFFStmt::NonBlockingAssign {
                        target: leg.get(&write_stage(&m.name, port, stage, suffix)),
                        value,
                    });
                }
            }
        }
    }
    out
}

/// Register declarations for both pipelines.
fn mem_pipeline_regs(
    memories: &[SHIRMemory],
    use_: &MemPortUse,
    leg: &Legalizer,
) -> Vec<VLIRRegDecl> {
    let mut out = Vec::new();
    for m in memories {
        for port in 0..m.read_ports {
            for (is_data, width) in [(true, width_of(&m.elem_ty)), (false, Width::Concrete(1))] {
                let Some(deepest) = read_chain_depth(m, port, use_, is_data) else { continue };
                for stage in 0..=deepest {
                    out.push(VLIRRegDecl {
                        name: leg.get(&read_stage(&m.name, port, stage, is_data)),
                        width: width.clone(),
                    });
                }
            }
        }
        for port in 0..m.write_ports {
            if !use_.writes.contains(&(m.name.clone(), port)) {
                continue;
            }
            for stage in 1..m.write_lat {
                for (suffix, width) in [
                    ("v", Width::Concrete(1)),
                    ("addr", addr_width(m.depth)),
                    ("data", width_of(&m.elem_ty)),
                ] {
                    out.push(VLIRRegDecl {
                        name: leg.get(&write_stage(&m.name, port, stage, suffix)),
                        width,
                    });
                }
            }
        }
    }
    out
}

/// The array declarations, their preloads, and the continuous read-data nets.
fn lower_mem_decls(
    memories: &[SHIRMemory],
    use_: &MemPortUse,
    leg: &Legalizer,
) -> LowerResult<Vec<VLIRMemDecl>> {
    memories
        .iter()
        // A RECEIVED memory has no child-side array: the array, its preload and
        // the continuous read-data assign are the OWNER's; the read-data net is
        // an input port instead (synthesized in `lower_to_vlir`).
        .filter(|m| !m.received)
        .map(|m| Ok(VLIRMemDecl {
            name: leg.get(&m.name),
            width: width_of(&m.elem_ty),
            depth: m.depth,
            read_data_nets: (0..m.read_ports)
                .filter(|p| use_.needs_data_net(&(m.name.clone(), *p)))
                .map(|p| VLIRMemReadNet {
                    data: leg.get(&mem_net(&m.name, true, p, "data")),
                    width: width_of(&m.elem_ty),
                    value: read_net_value(m, p, use_, leg),
                })
                .collect(),
            init: match &m.init {
                None => None,
                Some(SHIRMemInit::Fill { var, value }) => Some(VLIRMemInit::Fill {
                    var: var.clone(),
                    value: lower_expr(value, leg, &MemBinding::new())?,
                }),
                Some(SHIRMemInit::Words(words)) => Some(VLIRMemInit::Words(
                    words
                        .iter()
                        .map(|w| lower_expr(w, leg, &MemBinding::new()))
                        .collect::<LowerResult<_>>()?,
                )),
            },
        }))
        .collect()
}

/// What a read port's output net is driven from.
///
/// **ReadFirst** is a plain continuous read of the array. The write commits at the
/// edge with a non-blocking assign, so the read sees the pre-write contents for
/// free — no logic needed, which is why it was the first mode supported.
///
/// **WriteFirst** must forward this cycle's write to the read when the addresses
/// match, because the array itself will not hold the new value until after the
/// edge. That is a priority mux over the write ports, **highest index first**: the
/// simulator commits writes in ascending port order (`for port in 0..W`), so a
/// later port overwrites an earlier one and the highest index is what a read
/// observes. `tests/memory_multiport_arbitration.rs` establishes that rule at four
/// ports, where "highest index", "second one" and "last issued" finally disagree.
fn read_net_value(m: &SHIRMemory, port: usize, use_: &MemPortUse, leg: &Legalizer) -> VLIRExpr {
    let array_read = VLIRExpr::MemIndex {
        mem: leg.get(&m.name),
        addr: Box::new(VLIRExpr::Var(leg.get(&mem_net(&m.name, true, port, "addr")))),
    };
    if m.write_mode == WriteMode::ReadFirst {
        return array_read;
    }

    // Wrap ascending, so port 0 ends up innermost and the highest index outermost
    // — i.e. checked first, which is the priority the simulator implements.
    let mut value = array_read;
    for wp in 0..m.write_ports {
        if !use_.writes.contains(&(m.name.clone(), wp)) {
            continue;
        }
        // Forward from the stage that COMMITS this edge, which at a deeper write
        // latency is a pipeline register rather than the freshly staged nets.
        let (wen, waddr, wdata) = write_commit_nets(&m.name, wp, m.write_lat);
        let same_addr = VLIRExpr::BinOp {
            left: Box::new(VLIRExpr::Var(leg.get(&waddr))),
            op: VLIRBinOp::Eq,
            right: Box::new(VLIRExpr::Var(leg.get(&mem_net(&m.name, true, port, "addr")))),
        };
        value = VLIRExpr::Ternary {
            cond: Box::new(VLIRExpr::BinOp {
                left: Box::new(VLIRExpr::Var(leg.get(&wen))),
                op: VLIRBinOp::LogicalAnd,
                right: Box::new(same_addr),
            }),
            then_val: Box::new(VLIRExpr::Var(leg.get(&wdata))),
            else_val: Box::new(value),
        };
    }
    value
}

/// `if (<commit en>) <mem>[<commit addr>] <= <commit data>;` — one per staged
/// write port, from whichever stage commits this edge (the combinational nets at
/// `WRITE_LAT == 1`, the last pipeline stage otherwise).
///
/// Unguarded by phase: the enable is 0 outside the staging phase, so the guard is
/// implicit, and the pipeline must keep shifting on every edge regardless — the
/// simulator's does.
fn mem_write_commits(memories: &[SHIRMemory], use_: &MemPortUse, leg: &Legalizer) -> Vec<VLIRFFStmt> {
    let mut out = Vec::new();
    for m in memories {
        if m.received {
            // The commit is the OWNER's: the child only drives the bus.
            continue;
        }
        for port in 0..m.write_ports {
            if !use_.writes.contains(&(m.name.clone(), port)) {
                continue;
            }
            let (en, addr, data) = write_commit_nets(&m.name, port, m.write_lat);
            out.push(VLIRFFStmt::If {
                condition: VLIRExpr::Var(leg.get(&en)),
                then_stmts: vec![VLIRFFStmt::MemAssign {
                    mem: leg.get(&m.name),
                    addr: VLIRExpr::Var(leg.get(&addr)),
                    value: VLIRExpr::Var(leg.get(&data)),
                }],
                else_stmts: None,
            });
        }
    }
    out
}

/// See the call site: rewrite every `MemIndex` in `always_ff` position into the
/// committing-stage forwarding mux, per write port, highest index outermost
/// (the priority `read_net_value` establishes). Received memories are skipped —
/// the child has no array to index.
fn forward_ff_mem_index(
    stmts: &mut [VLIRFFStmt],
    memories: &[SHIRMemory],
    use_: &MemPortUse,
    leg: &Legalizer,
) {
    fn rewrite_expr(
        e: &mut VLIRExpr,
        memories: &[SHIRMemory],
        use_: &MemPortUse,
        leg: &Legalizer,
    ) {
        // Children first, so a nested MemIndex inside a rewritten mux's address
        // is handled exactly once.
        match e {
            VLIRExpr::BinOp { left, right, .. } => {
                rewrite_expr(left, memories, use_, leg);
                rewrite_expr(right, memories, use_, leg);
            }
            VLIRExpr::UnOp { expr, .. }
            | VLIRExpr::Slice { expr, .. }
            | VLIRExpr::Resize { expr, .. }
            | VLIRExpr::SignCast { expr, .. } => rewrite_expr(expr, memories, use_, leg),
            VLIRExpr::Ternary { cond, then_val, else_val } => {
                rewrite_expr(cond, memories, use_, leg);
                rewrite_expr(then_val, memories, use_, leg);
                rewrite_expr(else_val, memories, use_, leg);
            }
            VLIRExpr::Concat(parts) => {
                for p in parts {
                    rewrite_expr(p, memories, use_, leg);
                }
            }
            VLIRExpr::DynBit { base, index } => {
                rewrite_expr(base, memories, use_, leg);
                rewrite_expr(index, memories, use_, leg);
            }
            VLIRExpr::MemIndex { mem, addr } => {
                rewrite_expr(addr, memories, use_, leg);
                let Some(m) = memories
                    .iter()
                    .find(|m| !m.received && leg.get(&m.name) == *mem)
                else {
                    return;
                };
                let read_addr = (**addr).clone();
                let mut value = e.clone();
                for port in 0..m.write_ports {
                    if !use_.writes.contains(&(m.name.clone(), port)) {
                        continue;
                    }
                    let (en, waddr, wdata) = write_commit_nets(&m.name, port, m.write_lat);
                    let addr_expr = read_addr.clone();
                    value = VLIRExpr::Ternary {
                        cond: Box::new(VLIRExpr::BinOp {
                            left: Box::new(VLIRExpr::Var(leg.get(&en))),
                            op: VLIRBinOp::LogicalAnd,
                            right: Box::new(VLIRExpr::BinOp {
                                left: Box::new(addr_expr),
                                op: VLIRBinOp::Eq,
                                right: Box::new(VLIRExpr::Var(leg.get(&waddr))),
                            }),
                        }),
                        then_val: Box::new(VLIRExpr::Var(leg.get(&wdata))),
                        else_val: Box::new(value),
                    };
                }
                *e = value;
            }
            VLIRExpr::Var(_) | VLIRExpr::Lit { .. } => {}
        }
    }
    fn rewrite_stmt(
        st: &mut VLIRFFStmt,
        memories: &[SHIRMemory],
        use_: &MemPortUse,
        leg: &Legalizer,
    ) {
        match st {
            VLIRFFStmt::NonBlockingAssign { value, .. } => {
                rewrite_expr(value, memories, use_, leg)
            }
            VLIRFFStmt::MemAssign { addr, value, .. } => {
                rewrite_expr(addr, memories, use_, leg);
                rewrite_expr(value, memories, use_, leg);
            }
            VLIRFFStmt::If { condition, then_stmts, else_stmts } => {
                rewrite_expr(condition, memories, use_, leg);
                for s in then_stmts {
                    rewrite_stmt(s, memories, use_, leg);
                }
                if let Some(e) = else_stmts {
                    for s in e {
                        rewrite_stmt(s, memories, use_, leg);
                    }
                }
            }
            VLIRFFStmt::Case { selector, arms, default } => {
                rewrite_expr(selector, memories, use_, leg);
                for a in arms {
                    rewrite_expr(&mut a.selector_value, memories, use_, leg);
                    for s in &mut a.stmts {
                        rewrite_stmt(s, memories, use_, leg);
                    }
                }
                if let Some(d) = default {
                    for s in d {
                        rewrite_stmt(s, memories, use_, leg);
                    }
                }
            }
        }
    }
    for st in stmts {
        rewrite_stmt(st, memories, use_, leg);
    }
}

// ── Width helper ────────────────────────────────────────────────────────────

/// The outer packed dimension of an array type — `Some(ELS)` for
/// `[Bits<W>; ELS]`, `None` for anything else. Paired with `width_of`, which
/// reports the element width, this gives the two dimensions of the emitted
/// `[ELS-1:0][W-1:0]` declaration without either needing width arithmetic.
fn outer_dim_of(ty: &CHIRType) -> Option<Width> {
    match ty {
        CHIRType::Array { len, .. } => Some(len.clone()),
        _ => None,
    }
}

fn width_of(ty: &CHIRType) -> Width {
    match ty {
        CHIRType::UInt { width } | CHIRType::SInt { width } => width.clone(),
        CHIRType::Bool => Width::Concrete(1),
        // Element width — see `width_of_type`.
        CHIRType::Array { elem, .. } => width_of(elem),
    }
}

// ── Name legalization (Pass 1) ──────────────────────────────────────────────

struct Legalizer {
    map: std::cell::RefCell<HashMap<String, String>>,
    used: std::cell::RefCell<HashSet<String>>,
}

impl Legalizer {
    fn new() -> Self {
        Legalizer {
            map: std::cell::RefCell::new(HashMap::new()),
            used: std::cell::RefCell::new(HashSet::new()),
        }
    }

    /// Legalize `name`, caching the result. Idempotent per original name.
    fn legalize(&mut self, name: &str) -> String {
        if let Some(existing) = self.map.borrow().get(name) {
            return existing.clone();
        }
        let mut base = sanitize(name);
        if is_reserved(&base) {
            base.push_str("_sig");
        }
        // Disambiguate collisions after sanitization.
        let mut candidate = base.clone();
        let mut n = 0;
        while self.used.borrow().contains(&candidate) {
            candidate = format!("{base}_{n}");
            n += 1;
        }
        self.used.borrow_mut().insert(candidate.clone());
        self.map.borrow_mut().insert(name.to_string(), candidate.clone());
        candidate
    }

    /// Look up an already-legalized name; falls back to sanitizing on the fly
    /// for names that were not pre-registered (should not happen for valid IR).
    fn get(&self, name: &str) -> String {
        self.map.borrow().get(name).cloned().unwrap_or_else(|| sanitize(name))
    }
}

fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if out.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        out = format!("sig_{out}");
    }
    out
}

/// The emitted SystemVerilog name for a **port**, as `Legalizer::legalize` would
/// produce it: sanitized, with `_sig` appended when the name is reserved.
///
/// Exposed because a testbench addresses the Verilated model by the EMITTED name,
/// not the one in the Rust signature — `examples/cdc/flag_crossing.rs`'s `event`
/// port is `event_sig` in the SystemVerilog. The corpus differential sweep
/// (`build.rs`) generates that wiring and must agree with the transpiler about it,
/// so it calls THIS rather than reimplementing the rule; two copies of a naming
/// rule that must agree is the drift bug this repo keeps recording.
///
/// Collision disambiguation (`_0`, `_1`) is deliberately not modelled: it depends on
/// what else has been registered, which a caller outside a lowering run cannot know.
/// Ports are legalized first and from a small set, so a collision would have to be
/// between two ports of one module — and it fails loudly (an unknown member in the
/// generated C++) rather than silently.
pub fn legalized_port_name(name: &str) -> String {
    let mut base = sanitize(name);
    if is_reserved(&base) {
        base.push_str("_sig");
    }
    base
}

fn collect_and_legalize_body_names(body: &SHIRBody, leg: &mut Legalizer) {
    match body {
        SHIRBody::Combinational(c) => {
            for s in &c.submodules {
                leg.legalize(&s.inst_name);
                leg.legalize(&s.module_name);
                leg.legalize(&s.output_wire);
            }
            collect_stmt_names(&c.stmts, leg);
        }
        SHIRBody::Sequential(s) => {
            leg.legalize(&s.clock);
            for r in &s.registers {
                leg.legalize(&r.name);
            }
            for m in &s.submodules {
                leg.legalize(&m.inst_name);
                leg.legalize(&m.module_name);
                leg.legalize(&m.output_wire);
            }
            // The memory array and every net its ports could drive, reserved up
            // front so a user signal cannot collide with a synthesized name.
            for m in &s.memories {
                leg.legalize(&m.name);
                for (is_read, count) in [(true, m.read_ports), (false, m.write_ports)] {
                    for port in 0..count {
                        for suffix in ["en", "addr", "data"] {
                            leg.legalize(&mem_net(&m.name, is_read, port, suffix));
                        }
                        if is_read {
                            for stage in 0..m.read_lat {
                                leg.legalize(&read_stage(&m.name, port, stage, true));
                                leg.legalize(&read_stage(&m.name, port, stage, false));
                            }
                        } else {
                            for stage in 1..m.write_lat {
                                for sfx in ["v", "addr", "data"] {
                                    leg.legalize(&write_stage(&m.name, port, stage, sfx));
                                }
                            }
                        }
                    }
                }
            }
            for phase in &s.phases {
                collect_stmt_names(&phase.pre_edge, leg);
            }
        }
        SHIRBody::Structural(st) => {
            for (net, _) in &st.nets {
                leg.legalize(net);
            }
            for m in &st.submodules {
                leg.legalize(&m.inst_name);
                leg.legalize(&m.module_name);
                for (port, sig) in m.clocks.iter().chain(m.port_nets.iter()) {
                    leg.legalize(port);
                    leg.legalize(sig);
                }
            }
        }
    }
}

fn collect_stmt_names(stmts: &[SHIRStmt], leg: &mut Legalizer) {
    for s in stmts {
        match s {
            SHIRStmt::Wire { name, .. } => {
                leg.legalize(name);
            }
            SHIRStmt::PortDrive { .. } => {}
            SHIRStmt::If { then_stmts, else_stmts, .. } => {
                collect_stmt_names(then_stmts, leg);
                if let Some(e) = else_stmts {
                    collect_stmt_names(e, leg);
                }
            }
            SHIRStmt::Match { arms, .. } => {
                for a in arms {
                    collect_stmt_names(&a.stmts, leg);
                }
            }
            SHIRStmt::ForLoop { body, .. } => collect_stmt_names(body, leg),
            // A bit-assign targets an already-declared signal; nothing new to name.
            // Memory nets are reserved from the memory declaration, not from the
            // access sites (an unused port declares nothing).
            SHIRStmt::IndexAssign { .. }
            | SHIRStmt::MemRead { .. }
            | SHIRStmt::MemWrite { .. } => {}
        }
    }
}

// Unused for M1 phases but referenced by the multi-phase path.
#[allow(dead_code)]
fn _phase_marker(_: &SHIRPhase) {}

/// Verilog / SystemVerilog reserved keywords (VLIR_DESIGN §Pass 1), plus the C++
/// words Verilator will not accept in a generated model.
///
/// A name that lands here gets `_sig` appended by [`Legalizer::legalize`], so a
/// legal Copper identifier never becomes an illegal SystemVerilog one.
///
/// **Both lists are load-bearing, and both had holes.** `examples/cdc/flag_crossing.rs`
/// has a port named `event` — a SystemVerilog keyword absent from the list below
/// until 2026-08-25 — and emitted SystemVerilog that Verilator could not parse
/// (*"syntax error, unexpected event"*). It went unnoticed because that example is
/// checked against hand-written Verilog and nothing had ever Verilated what it
/// transpiles to; `tests/corpus_equivalence.rs` is what found it. The C++ half is
/// the same class one step further out: Verilator emits a C++ model, so a port named
/// `abort` or `delete` builds clean SystemVerilog and then fails the C++ compile
/// (`SYMRSVDWORD`), which is fatal under the harness's `-Wall`.
///
/// Adding a word only ever renames an identifier that was already unusable, so the
/// lists are kept deliberately complete rather than minimal. Measured when they were
/// filled in: exactly one name in the whole corpus changed (`event`).
fn is_reserved(name: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "always", "always_comb", "always_ff", "always_latch", "and", "assign", "automatic",
        "begin", "bit", "buf", "byte", "case", "casex", "casez", "cell", "clocking", "config",
        "default", "defparam", "disable", "do", "edge", "else", "end", "endcase", "endconfig",
        "endfunction", "endgenerate", "endgroup", "endinterface", "endmodule", "endpackage",
        "endprimitive", "endprogram", "endproperty", "endspecify", "endsequence", "endtable",
        "endtask", "enum", "export", "extends", "extern", "final", "for", "force", "foreach",
        "forever", "fork", "function", "generate", "genvar", "if", "iff", "import", "initial",
        "inout", "input", "instance", "int", "integer", "interface", "join", "local", "localparam",
        "logic", "longint", "macromodule", "modport", "module", "nand", "negedge", "new", "nor",
        "not", "or", "output", "package", "packed", "parameter", "posedge", "primitive", "priority",
        "program", "property", "real", "realtime", "ref", "reg", "release", "repeat", "return",
        "sequence", "shortint", "shortreal", "signed", "specify", "specparam", "static", "string",
        "struct", "super", "table", "task", "this", "time", "timeprecision", "timeunit", "tran",
        "tri", "type", "typedef", "union", "unique", "unsigned", "var", "virtual", "void", "wait",
        "wand", "while", "wire", "with", "wor", "xnor", "xor",
        // IEEE 1800-2017 additions, and the Verilog-1995/2001 gate and net keywords
        // the original list skipped.
        "accept_on", "alias", "assert", "assume", "before", "bind", "bins", "binsof",
        "break", "bufif0", "bufif1", "chandle", "checker", "class", "cmos", "const",
        "constraint", "context", "continue", "cover", "covergroup", "coverpoint",
        "cross", "deassign", "dist", "endchecker", "endclass", "endclocking", "event",
        "eventually", "expect", "first_match", "forkjoin", "global", "highz0",
        "highz1", "ifnone", "ignore_bins", "illegal_bins", "implements", "implies",
        "incdir", "include", "inside", "interconnect", "intersect", "join_any",
        "join_none", "large", "let", "liblist", "library", "matches", "medium",
        "nettype", "nexttime", "nmos", "noshowcancelled", "notif0", "notif1", "null",
        "pmos", "pull0", "pull1", "pulldown", "pullup", "pulsestyle_ondetect",
        "pulsestyle_onevent", "pure", "rand", "randc", "randcase", "randsequence",
        "rcmos", "reject_on", "restrict", "rnmos", "rpmos", "rtran", "rtranif0",
        "rtranif1", "s_always", "s_eventually", "s_nexttime", "s_until",
        "s_until_with", "scalared", "showcancelled", "small", "soft", "solve",
        "strong", "strong0", "strong1", "supply0", "supply1", "sync_accept_on",
        "sync_reject_on", "tagged", "throughout", "tranif0", "tranif1", "tri0",
        "tri1", "triand", "trior", "trireg", "unique0", "until", "until_with",
        "untyped", "use", "uwire", "vectored", "wait_order", "weak", "weak0",
        "weak1", "wildcard", "within",
    ];
    /// C++ keywords and the few common words Verilator refuses (`SYMRSVDWORD`),
    /// because the model it generates is C++ and the port becomes a member name.
    /// The SystemVerilog itself is legal — this is about what can be BUILT.
    const CPP_RESERVED: &[&str] = &[
        "abort", "alignas", "alignof", "asm", "auto", "bool", "catch", "char",
        "char8_t", "char16_t", "char32_t", "co_await", "co_return", "co_yield",
        "compl", "concept", "const_cast", "consteval", "constexpr", "constinit",
        "decltype", "delete", "double", "dynamic_cast", "explicit", "false", "float",
        "friend", "goto", "inline", "long", "main", "mutable", "namespace",
        "noexcept", "nullptr", "operator", "private", "protected", "public",
        "register", "reinterpret_cast", "requires", "short", "sizeof",
        "static_assert", "static_cast", "std", "switch", "template", "throw", "true",
        "try", "typeid", "typename", "using", "volatile", "wchar_t",
    ];
    KEYWORDS.contains(&name) || CPP_RESERVED.contains(&name)
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::{transpile_item_fn, EmitConfig};
    use std::collections::{HashMap, HashSet};

    fn transpile(src: &str) -> String {
        let f: syn::ItemFn = syn::parse_str(src).expect("parse");
        transpile_item_fn(&f, &HashSet::new(), &HashMap::new(), &EmitConfig::default())
            .expect("transpile")
    }

    /// A port whose name is reserved is renamed, not emitted verbatim — otherwise
    /// the module is syntactically invalid (SystemVerilog keyword) or cannot be
    /// built (C++ keyword, which Verilator rejects as `SYMRSVDWORD`).
    ///
    /// `event` is the real instance: `examples/cdc/flag_crossing.rs` shipped it, and
    /// the emitted SystemVerilog would not parse — found 2026-08-25 by
    /// `tests/corpus_equivalence.rs`, the first thing ever to Verilate what that
    /// example transpiles to.
    #[test]
    fn a_reserved_port_name_is_legalized() {
        let src = r#"
            async fn m(clk: Clock<MainClk>, trigger: In<Logic, MainClk>,
                       event: Out<Logic, MainClk>, delete: Out<Logic, MainClk>) {
                loop {
                    event.write(trigger.read());
                    delete.write(trigger.read());
                    clk.tick().await;
                }
            }
        "#;
        let sv = transpile(src);
        assert!(
            sv.contains("output logic event_sig") && sv.contains("output logic delete_sig"),
            "a reserved port name must be legalized — `event` is a SystemVerilog \
             keyword and `delete` a C++ one, and neither can be emitted verbatim:\n{sv}"
        );
        assert!(
            !sv.contains("output logic event;") && !sv.contains("output logic delete;"),
            "the bare reserved name is still emitted somewhere:\n{sv}"
        );
    }

    #[test]
    fn multi_phase_wires_get_top_defaults_no_latch() {
        // A 3-phase pipeline (mac): `product`/`c_s` computed in phase 0, `sum` in
        // phase 1, output in phase 2. Each comb temp is phase-local; without a top
        // default the merged always_comb would infer a latch (Verilator LATCH).
        let src = r#"
            async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, b: In<Bits<8>, MainClk>,
                       c: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) {
                loop {
                    let product = a.read() * b.read();
                    let c_s = c.read();
                    clk.tick().await;
                    let sum = product + c_s;
                    clk.tick().await;
                    out.write(sum);
                    clk.tick().await;
                }
            }
        "#;
        let sv = transpile(src);
        let comb = &sv[sv.find("always_comb").unwrap()..sv.find("always_ff").unwrap()];
        // Phase-local temps are defaulted before the phase guards.
        assert!(comb.contains("product = '0;"), "expected phase-local defaults:\n{sv}");
        assert!(comb.contains("sum = '0;"), "expected phase-local defaults:\n{sv}");
    }

    #[test]
    fn multiply_driven_output_across_phases_is_rejected() {
        // `out` driven in two phases would emit two conflicting continuous assigns.
        let src = r#"
            async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) {
                loop {
                    out.write(a.read());
                    clk.tick().await;
                    out.write(Bits::from_u8(0));
                    clk.tick().await;
                }
            }
        "#;
        let f: syn::ItemFn = syn::parse_str(src).expect("parse");
        let res = transpile_item_fn(&f, &HashSet::new(), &HashMap::new(), &EmitConfig::default());
        assert!(res.is_err(), "multiply-driven output must be rejected");
    }

    #[test]
    fn auto_hoist_block_local_default_avoids_false_latch() {
        // `tmp` is declared with a default and used only inside the `en` branch —
        // a block-local temporary. Without the auto-hoist it's a false latch
        // (assigned on some paths, but only read where assigned). The hoist moves
        // its default to the top of always_comb, so it transpiles from unmodified,
        // natural Rust.
        let src = r#"
            fn m(a: In<Bits<8>, ()>, en: In<Logic, ()>, out: Out<Bits<8>, ()>) {
                let mut r: Bits<8> = Bits::from_u8(0);
                if en.read() == Logic::One {
                    let mut tmp: Bits<8> = Bits::from_u8(0);
                    tmp = a.read();
                    r = tmp;
                }
                out.write(r);
            }
        "#;
        let sv = transpile(src);
        // `tmp`'s default is hoisted above the branch (appears before the `if`).
        let comb = &sv[sv.find("always_comb").unwrap()..];
        let tmp_pos = comb.find("tmp = 8'd0;").expect("hoisted tmp default");
        let if_pos = comb.find("if (").expect("branch");
        assert!(tmp_pos < if_pos, "tmp default should be hoisted before the branch:\n{sv}");
    }

    #[test]
    fn for_loop_with_reassignment_emits_end_to_end() {
        // The loop body reassigns a combinational local — this is what makes a
        // for-loop produce real hardware (previously the reassign was dropped).
        let src = r#"
            fn accumulate<const N: usize>(a: In<Bits<32>, ()>, out: Out<Bits<32>, ()>) {
                let mut acc: Bits<32> = a.read();
                for i in 0..N {
                    acc = acc + Bits::from_u32(1);
                }
                out.write(acc);
            }
        "#;
        let sv = transpile(src);
        println!("\n===== GENERATED VERILOG (for + reassign) =====\n{sv}\n=====");
        assert!(sv.contains("; i < N; i++) begin"), "expected SV for loop: {sv}");
        assert!(sv.contains("acc = (acc + 32'd1);"), "expected blocking reassign: {sv}");
        // `acc` is declared exactly once despite two assignments to it.
        assert_eq!(sv.matches("logic [31:0] acc;").count(), 1, "acc declared once: {sv}");
    }

    #[test]
    fn lhs_bit_assign_with_dynamic_index_emits() {
        // `o[i] = a[i]` inside a loop: LHS bit-assign + dynamic bit-select read.
        let src = r#"
            fn copy_bits<const N: usize>(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) {
                let mut o: Bits<8> = Bits::from_u8(0);
                for i in 0..N {
                    o[i] = a.read()[i];
                }
                out.write(o);
            }
        "#;
        let sv = transpile(src);
        println!("\n===== GENERATED VERILOG (bit-assign) =====\n{sv}\n=====");
        assert!(sv.contains("o[i] = a[i];"), "expected LHS bit-assign + dyn read: {sv}");
    }

    #[test]
    fn const_generic_module_emits_sv_parameter() {
        // A const-generic `Bits<N>` module emits a SystemVerilog `parameter` and
        // parametric `[N-1:0]` port ranges (M2 const-generic increment 1).
        let src = r#"
            fn passthru<const N: usize>(a: In<Bits<N>, C>, out: Out<Bits<N>, C>) {
                out.write(a.read());
            }
        "#;
        let sv = transpile(src);
        println!("\n===== GENERATED VERILOG (const-generic) =====\n{sv}\n=====");
        assert!(sv.contains("module passthru #("), "expected parameter header: {sv}");
        assert!(sv.contains("parameter int N"), "expected `parameter int N`: {sv}");
        assert!(sv.contains("[N-1:0]"), "expected parametric range `[N-1:0]`: {sv}");
    }

    #[test]
    fn counter_emits_verilog() {
        let src = r#"
            async fn counter(clk: Clock<MainClk>, step: In<u8, MainClk>, out: Out<u8, MainClk>) {
                let mut count = 0u8;
                loop {
                    out.write(count);
                    clk.tick().await;
                    count = count.wrapping_add(step.read());
                }
            }
        "#;
        let sv = transpile(src);
        println!("\n===== GENERATED VERILOG (counter) =====\n{sv}=======================================");
        assert!(sv.contains("module counter"));
        assert!(sv.contains("always_ff @(posedge clk)"));
        assert!(sv.contains("assign out ="));
        assert!(sv.contains("<="));
    }

    /// Golden test: exact emitted text for the canonical counter. Catches
    /// unintended formatting/ordering churn in the emitter. Update the expected
    /// string deliberately when the emitter output is meant to change.
    #[test]
    fn counter_golden_output() {
        let src = r#"
            async fn counter(clk: Clock<MainClk>, step: In<u8, MainClk>, out: Out<u8, MainClk>) {
                let mut count = 0u8;
                loop {
                    out.write(count);
                    clk.tick().await;
                    count = count.wrapping_add(step.read());
                }
            }
        "#;
        let expected = "\
module counter (
    input  logic clk,
    input  logic [7:0] step,
    output logic [7:0] out
);

    logic [7:0] count;

    always_ff @(posedge clk) begin
        count <= (count + step);
    end

    assign out = count;

endmodule
";
        assert_eq!(transpile(src), expected);
    }

    /// Combinational `Logic` module: `.read()` + logic operators infer 1-bit
    /// widths, and intermediate wires are declared. Regression for the M2
    /// width-inference fix. (Previously failed with `AmbiguousWidth`.)
    #[test]
    fn logic_comb_module_golden_output() {
        let src = r#"
            fn one_bit_comparator(i0: In<Logic, ()>, i1: In<Logic, ()>, eq: Out<Logic, ()>) {
                let p0 = !i0.read() & !i1.read();
                let p1 = i0.read() & i1.read();
                eq.write(p0 | p1);
            }
        "#;
        let expected = "\
module one_bit_comparator (
    input  logic i0,
    input  logic i1,
    output logic eq
);

    logic p0;
    logic p1;

    always_comb begin
        p0 = ((!i0) & (!i1));
        p1 = (i0 & i1);
    end

    assign eq = (p0 | p1);

endmodule
";
        assert_eq!(transpile(src), expected);
    }

    /// Bit-indexing (`d[7]`) + `Logic::One` constant + conditional register
    /// update lower end-to-end to a bit-select, comparison, and ternary.
    #[test]
    fn bit_index_golden_output() {
        let src = r#"
            async fn m(clk: Clock<MainClk>, d: In<Bits<8>, MainClk>, o: Out<Bits<8>, MainClk>) {
                let mut r = Bits::from_u8(0);
                loop {
                    o.write(r);
                    clk.tick().await;
                    if d.read()[7] == Logic::One {
                        r = d.read();
                    }
                }
            }
        "#;
        let expected = "\
module m (
    input  logic clk,
    input  logic [7:0] d,
    output logic [7:0] o
);

    logic [7:0] r;

    always_ff @(posedge clk) begin
        r <= ((d[7] == 1'b1) ? d : r);
    end

    assign o = r;

endmodule
";
        assert_eq!(transpile(src), expected);
    }

    /// A `let`-bound `if`-expression with `{ block }` branches: the register
    /// update infers its width from a branch's block tail, and both branches
    /// lower via the block-tail extractor into a ternary.
    #[test]
    fn if_expr_with_block_branches_golden_output() {
        let src = r#"
            async fn m(clk: Clock<MainClk>, d: In<Bits<8>, MainClk>, sel: In<Logic, MainClk>, o: Out<Bits<8>, MainClk>) {
                let mut r = Bits::from_u8(0);
                loop {
                    o.write(r);
                    clk.tick().await;
                    r = if sel.read().as_bool() {
                        (d.read() >> 1) ^ r
                    } else {
                        d.read() >> 1
                    };
                }
            }
        "#;
        let expected = "\
module m (
    input  logic clk,
    input  logic [7:0] d,
    input  logic sel,
    output logic [7:0] o
);

    logic [7:0] r;

    always_ff @(posedge clk) begin
        r <= (sel ? ((d >> 8'd1) ^ r) : (d >> 8'd1));
    end

    assign o = r;

endmodule
";
        assert_eq!(transpile(src), expected);
    }

    /// Enum-as-state: variants encode to concrete values, the enum register is
    /// sized to hold them, `match` becomes a `case`, and a conditional (Moore)
    /// output lowers into `always_comb` with an assignment on every path.
    #[test]
    fn enum_state_machine_golden_output() {
        let src = r#"
            enum State { IDLE = 0, RUN = 1, DONE = 2 }

            #[hardware(sequential)]
            async fn fsm(clk: Clock<MainClk>, o: Out<Logic, MainClk>) {
                let mut state = State::IDLE;
                loop {
                    if state == State::DONE {
                        o.write(Logic::One);
                    } else {
                        o.write(Logic::Zero);
                    }
                    clk.tick().await;
                    state = match state {
                        State::IDLE => State::RUN,
                        State::RUN => State::DONE,
                        _ => State::IDLE,
                    };
                }
            }
        "#;
        let expected = "\
module fsm (
    input  logic clk,
    output logic o
);

    logic [1:0] state;

    always_comb begin
        if ((state == 2'd2)) begin
            o = 1'b1;
        end else begin
            o = 1'b0;
        end
    end

    always_ff @(posedge clk) begin
        case (state)
            2'd0: begin
                state <= 2'd1;
            end
            2'd1: begin
                state <= 2'd2;
            end
            default: begin
                state <= 2'd0;
            end
        endcase
    end

endmodule
";
        // Uses transpile_source so the file-scope `enum State` is injected.
        let sv = crate::transpile_source(src, None, &EmitConfig::default()).expect("transpile");
        assert_eq!(sv, expected);
    }

    /// Tuple patterns encode to a single concatenated selector value, first
    /// element most-significant — matching the `Concat` scrutinee.
    #[test]
    fn tuple_pattern_concat_encoding() {
        use copper_core::chir::CHIRType;
        use copper_core::shir::{SHIRLit, SHIRPattern};
        let lit = |w: usize, v: u128| {
            SHIRPattern::Lit(SHIRLit { ty: CHIRType::UInt { width: Width::Concrete(w) }, value: v })
        };

        let leg = Legalizer::new();
        // (State=2 :: 3 bits, in=0 :: 1 bit) -> {010, 0} = 4'd4
        let p = SHIRPattern::Tuple(vec![lit(3, 2), lit(1, 0)]);
        match pattern_to_selector(&p, &leg).expect("selector") {
            VLIRExpr::Lit { width, value } => {
                assert_eq!(width, Width::Concrete(4));
                assert_eq!(value, 4);
            }
            other => panic!("expected literal selector, got {other:?}"),
        }

        // (State=5 :: 3 bits, in=1 :: 1 bit) -> {101, 1} = 4'd11
        let p = SHIRPattern::Tuple(vec![lit(3, 5), lit(1, 1)]);
        match pattern_to_selector(&p, &leg).expect("selector") {
            VLIRExpr::Lit { value, .. } => assert_eq!(value, 11),
            other => panic!("expected literal selector, got {other:?}"),
        }

        // A wildcard inside a tuple has no single selector value → rejected.
        let p = SHIRPattern::Tuple(vec![lit(3, 1), SHIRPattern::Wildcard]);
        assert!(pattern_to_selector(&p, &leg).is_err());
    }

    /// Latch inference is rejected, not emitted. Copper's premise is that this
    /// class of bug should be impossible to express, so a combinational signal
    /// assigned on only some control paths is a hard error.
    #[test]
    fn conditional_output_becomes_registered_not_latch() {
        // `out` is written only in the `Stage::Out` arm. The simulator holds an
        // output between writes, so this is an implicit-hold register, not a
        // combinational latch: it lowers to a guarded `always_ff` update
        // (`case (stage) ... out <= acc; default: ; endcase`) that holds
        // otherwise — not the old hard latch error.
        let src = r#"
            enum Stage { Load = 0, Mul = 1, Out = 2 }

            #[hardware(sequential)]
            async fn m(clk: Clock<MainClk>, a: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) {
                let mut stage = Stage::Load;
                let mut acc: Bits<8> = Bits::from_lit::<0>();
                loop {
                    match stage {
                        Stage::Load => { acc = a.read(); stage = Stage::Mul; }
                        Stage::Mul => { stage = Stage::Out; }
                        _ => { out.write(acc); stage = Stage::Load; }
                    }
                    clk.tick().await;
                }
            }
        "#;
        let sv = crate::transpile_source(src, None, &EmitConfig::default())
            .expect("a conditionally-written output registers (implicit hold), not a latch");
        // The output is a registered non-blocking assign in always_ff, and is NOT
        // driven combinationally (no `out = ` blocking assign).
        assert!(sv.contains("out <="), "expected a registered output: {sv}");
        assert!(!sv.contains("out ="), "output must not be a combinational drive: {sv}");
    }

    /// ...but a `case` whose arms cover every value of the selector is complete,
    /// so it must NOT be flagged (this is the traffic-light Moore-output shape:
    /// four arms over a 2-bit enum, no `default`).
    #[test]
    fn exhaustive_case_is_not_flagged_as_latch() {
        let src = r#"
            enum P { A = 0, B = 1, C = 2, D = 3 }

            #[hardware(sequential)]
            async fn m(clk: Clock<MainClk>, o: Out<Logic, MainClk>) {
                let mut p = P::A;
                loop {
                    match p {
                        P::A => { o.write(Logic::Zero); }
                        P::B => { o.write(Logic::One); }
                        P::C => { o.write(Logic::Zero); }
                        P::D => { o.write(Logic::One); }
                    }
                    clk.tick().await;
                    p = match p {
                        P::A => P::B,
                        P::B => P::C,
                        P::C => P::D,
                        _ => P::A,
                    };
                }
            }
        "#;
        let sv = crate::transpile_source(src, None, &EmitConfig::default())
            .expect("an exhaustive case assigns on every path");
        assert!(sv.contains("always_comb"));
    }

    /// The deferred-feature guards produce errors, not wrong Verilog.
    #[test]
    fn tuple_match_lowers_to_case_over_concat() {
        // A match on a tuple scrutinee (once an M2-deferred feature) now lowers to
        // a `case` over the concatenated selector `{j, k}` — a correct JK
        // flip-flop (hold / reset / set / toggle). Its next-state `match` sits in
        // the post-tick (trailing) segment, so this also covers the trailing
        // segment's register updates.
        let src = r#"
            async fn jk(clk: Clock<C>, j: In<bool, C>, k: In<bool, C>, q: Out<bool, C>) {
                let mut state = false;
                loop {
                    q.write(state);
                    clk.tick().await;
                    state = match (j.read(), k.read()) {
                        (false, false) => state,
                        (false, true) => false,
                        (true, false) => true,
                        _ => !state,
                    };
                }
            }
        "#;
        let f: syn::ItemFn = syn::parse_str(src).expect("parse");
        let sv = transpile_item_fn(&f, &HashSet::new(), &HashMap::new(), &EmitConfig::default())
            .expect("tuple-scrutinee match now transpiles");
        assert!(sv.contains("case ({j, k})"), "expected case over {{j, k}}: {sv}");
        assert!(sv.contains("state <= (!state)"), "expected toggle in the j=k=1 arm: {sv}");
    }
}

// ── Sole-consumer index narrowing ────────────────────────────────────────────

/// Narrow a combinational wire whose **every** use is the direct child of a
/// same-width narrowing `Resize` to that width, resizing its defining
/// assignments to match.
///
/// This is the `wide_index_sole_consumer` decision, ruled 2026-08-27 ("emit the
/// index at the address width"): a `usize` index local is 32 bits while a
/// memory address net is `clog2(depth)`, so `10'(i)` reads `i[9:0]` and an
/// index that feeds NOTHING ELSE has a structurally dead upper half —
/// UNUSEDSIGNAL, fatal under the sweep's `-Wall`. When the dead half is
/// provable (every occurrence of the wire sits directly under a `Resize` to one
/// agreed narrower concrete width), the wire is declared at that width and each
/// of its assignments truncates explicitly — the same bits every consumer
/// already read. Any other use, conflicting widths, a widening resize, or a
/// symbolic width disqualifies the wire (`wide_index_into_narrow_addr`, whose
/// index is also read whole, stays 32 bits). Registers and ports are never
/// candidates.
fn narrow_sole_resize_wires(m: &mut VLIRModule) {
    use std::collections::HashMap;

    #[derive(Default)]
    struct Use {
        narrow: Option<usize>,
        conflicted: bool,
        other: bool,
    }

    fn scan_expr(e: &VLIRExpr, u: &mut HashMap<String, Use>) {
        if let VLIRExpr::Resize { expr, width: Width::Concrete(k) } = e {
            if let VLIRExpr::Var(name) = expr.as_ref() {
                let entry = u.entry(name.clone()).or_default();
                match entry.narrow {
                    None => entry.narrow = Some(*k),
                    Some(prev) if prev != *k => entry.conflicted = true,
                    _ => {}
                }
                return;
            }
        }
        match e {
            VLIRExpr::Var(name) => u.entry(name.clone()).or_default().other = true,
            VLIRExpr::Lit { .. } => {}
            VLIRExpr::BinOp { left, right, .. } => {
                scan_expr(left, u);
                scan_expr(right, u);
            }
            VLIRExpr::UnOp { expr, .. }
            | VLIRExpr::SignCast { expr, .. }
            | VLIRExpr::Slice { expr, .. }
            | VLIRExpr::Resize { expr, .. } => scan_expr(expr, u),
            VLIRExpr::Ternary { cond, then_val, else_val } => {
                scan_expr(cond, u);
                scan_expr(then_val, u);
                scan_expr(else_val, u);
            }
            VLIRExpr::Concat(parts) => {
                for p in parts {
                    scan_expr(p, u);
                }
            }
            VLIRExpr::DynBit { base, index } => {
                scan_expr(base, u);
                scan_expr(index, u);
            }
            VLIRExpr::MemIndex { addr, .. } => scan_expr(addr, u),
        }
    }

    fn scan_stmt(s: &VLIRStmt, u: &mut HashMap<String, Use>) {
        match s {
            VLIRStmt::WireAssign { value, .. } => scan_expr(value, u),
            VLIRStmt::PortAssign { value, edge_value, .. } => {
                scan_expr(value, u);
                scan_expr(edge_value, u);
            }
            VLIRStmt::If { condition, edge_condition, then_stmts, else_stmts } => {
                scan_expr(condition, u);
                scan_expr(edge_condition, u);
                for t in then_stmts {
                    scan_stmt(t, u);
                }
                if let Some(es) = else_stmts {
                    for t in es {
                        scan_stmt(t, u);
                    }
                }
            }
            VLIRStmt::Case { selector, edge_selector, arms, default } => {
                scan_expr(selector, u);
                scan_expr(edge_selector, u);
                for a in arms {
                    scan_expr(&a.selector_value, u);
                    for t in &a.stmts {
                        scan_stmt(t, u);
                    }
                }
                if let Some(d) = default {
                    for t in d {
                        scan_stmt(t, u);
                    }
                }
            }
            VLIRStmt::ForLoop { start, end, body, .. } => {
                scan_expr(start, u);
                scan_expr(end, u);
                for t in body {
                    scan_stmt(t, u);
                }
            }
            VLIRStmt::IndexAssign { index, value, .. } => {
                scan_expr(index, u);
                scan_expr(value, u);
            }
        }
    }

    fn scan_ff(s: &VLIRFFStmt, u: &mut HashMap<String, Use>) {
        match s {
            VLIRFFStmt::NonBlockingAssign { value, .. } => scan_expr(value, u),
            VLIRFFStmt::MemAssign { addr, value, .. } => {
                scan_expr(addr, u);
                scan_expr(value, u);
            }
            VLIRFFStmt::If { condition, then_stmts, else_stmts } => {
                scan_expr(condition, u);
                for t in then_stmts {
                    scan_ff(t, u);
                }
                if let Some(es) = else_stmts {
                    for t in es {
                        scan_ff(t, u);
                    }
                }
            }
            VLIRFFStmt::Case { selector, arms, default } => {
                scan_expr(selector, u);
                for a in arms {
                    scan_expr(&a.selector_value, u);
                    for t in &a.stmts {
                        scan_ff(t, u);
                    }
                }
                if let Some(d) = default {
                    for t in d {
                        scan_ff(t, u);
                    }
                }
            }
        }
    }

    fn decl_widths(s: &VLIRStmt, out: &mut HashMap<String, usize>) {
        match s {
            VLIRStmt::WireAssign { name, width: Width::Concrete(n), outer_dim: None, .. } => {
                out.entry(name.clone()).or_insert(*n);
            }
            VLIRStmt::If { then_stmts, else_stmts, .. } => {
                for t in then_stmts {
                    decl_widths(t, out);
                }
                if let Some(es) = else_stmts {
                    for t in es {
                        decl_widths(t, out);
                    }
                }
            }
            VLIRStmt::Case { arms, default, .. } => {
                for a in arms {
                    for t in &a.stmts {
                        decl_widths(t, out);
                    }
                }
                if let Some(d) = default {
                    for t in d {
                        decl_widths(t, out);
                    }
                }
            }
            VLIRStmt::ForLoop { body, .. } => {
                for t in body {
                    decl_widths(t, out);
                }
            }
            _ => {}
        }
    }

    fn rewrite(s: &mut VLIRStmt, winners: &HashMap<String, usize>) {
        match s {
            VLIRStmt::WireAssign { name, width, value, outer_dim: None } => {
                if let Some(&k) = winners.get(name) {
                    *width = Width::Concrete(k);
                    let old = std::mem::replace(value, VLIRExpr::Var(String::new()));
                    *value = VLIRExpr::Resize { width: Width::Concrete(k), expr: Box::new(old) };
                }
            }
            VLIRStmt::If { then_stmts, else_stmts, .. } => {
                for t in then_stmts {
                    rewrite(t, winners);
                }
                if let Some(es) = else_stmts {
                    for t in es {
                        rewrite(t, winners);
                    }
                }
            }
            VLIRStmt::Case { arms, default, .. } => {
                for a in arms {
                    for t in &mut a.stmts {
                        rewrite(t, winners);
                    }
                }
                if let Some(d) = default {
                    for t in d {
                        rewrite(t, winners);
                    }
                }
            }
            VLIRStmt::ForLoop { body, .. } => {
                for t in body {
                    rewrite(t, winners);
                }
            }
            _ => {}
        }
    }

    let VLIRBody::Sequential(seq) = &m.body else { return };

    let mut uses: HashMap<String, Use> = HashMap::new();
    for ph in &seq.comb_phases {
        if let Some(g) = &ph.phase_guard {
            scan_expr(g, &mut uses);
        }
        for st in &ph.stmts {
            scan_stmt(st, &mut uses);
        }
    }
    for st in &seq.always_ff.stmts {
        scan_ff(st, &mut uses);
    }
    for a in &seq.output_assigns {
        scan_expr(&a.value, &mut uses);
    }
    for mem in &seq.memories {
        for net in &mem.read_data_nets {
            scan_expr(&net.value, &mut uses);
        }
        match &mem.init {
            Some(VLIRMemInit::Fill { value, .. }) => scan_expr(value, &mut uses),
            Some(VLIRMemInit::Words(ws)) => {
                for w in ws {
                    scan_expr(w, &mut uses);
                }
            }
            None => {}
        }
    }
    for sub in &seq.submodules {
        for (_, e) in &sub.inputs {
            scan_expr(e, &mut uses);
        }
    }

    let regs: std::collections::HashSet<&str> =
        seq.reg_decls.iter().map(|r| r.name.as_str()).collect();

    let mut widths = HashMap::new();
    for ph in &seq.comb_phases {
        for st in &ph.stmts {
            decl_widths(st, &mut widths);
        }
    }
    let mut winners: HashMap<String, usize> = HashMap::new();
    for (name, decl_w) in widths {
        if regs.contains(name.as_str()) {
            continue;
        }
        if let Some(us) = uses.get(&name) {
            if let (Some(k), false, false) = (us.narrow, us.conflicted, us.other) {
                if k < decl_w {
                    winners.insert(name, k);
                }
            }
        }
    }
    if winners.is_empty() {
        return;
    }

    let VLIRBody::Sequential(seq) = &mut m.body else { return };
    for ph in &mut seq.comb_phases {
        for st in &mut ph.stmts {
            rewrite(st, &winners);
        }
    }
}
