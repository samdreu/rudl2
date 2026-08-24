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
use copper_core::shir::{
    SHIRBody, SHIRCombBody, SHIRExpr, SHIRLit, SHIRMemory, SHIRModule, SHIRPhase, SHIRPortDir, SHIRPortKind,
    SHIRRegUpdate, SHIRSeqBody, SHIRStmt, SHIRStructuralBody, SHIRSubmoduleInst,
};
use copper_core::vlir::{
    VLIRAlwaysFF, VLIRBinOp, VLIRCaseArm, VLIRBody, VLIRCombBody, VLIRCombPhase, VLIRContinuousAssign, VLIRExpr,
    VLIRFFCaseArm, VLIRFFStmt, VLIRMemDecl, VLIRMemReadNet, VLIRModule, VLIRPort, VLIRPortDir, VLIRPortKind, VLIRRegDecl, VLIRSeqBody, VLIRStmt,
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

    let ports = shir
        .ports
        .iter()
        .map(|p| VLIRPort {
            name: leg.get(&p.name),
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

    let params = shir
        .params
        .iter()
        .map(|p| copper_core::vlir::VLIRParam { name: p.name.clone(), default: p.default })
        .collect();

    Ok(VLIRModule { name, params, ports, body })
}

// ── Combinational body ──────────────────────────────────────────────────────

fn lower_comb(c: &SHIRCombBody, leg: &Legalizer) -> LowerResult<VLIRCombBody> {
    let submodules = c.submodules.iter().map(|s| lower_submodule(s, leg)).collect::<LowerResult<_>>()?;
    // A combinational module has no clock, so it has no registered outputs.
    let (mut comb_stmts, output_assigns) = lower_flat_stmts(&c.stmts, leg, &HashSet::new(), &MemWidths::default())?;
    hoist_branch_local_defaults(&mut comb_stmts);
    check_no_latches(&comb_stmts)?;
    Ok(VLIRCombBody { submodules, comb_stmts, output_assigns })
}

// ── Structural body ─────────────────────────────────────────────────────────

fn lower_structural(st: &SHIRStructuralBody, leg: &Legalizer) -> LowerResult<VLIRStructuralBody> {
    let nets = st.nets.iter().map(|(n, ty)| (leg.get(n), width_of(ty))).collect();
    let submodules = st.submodules.iter().map(|s| lower_submodule(s, leg)).collect::<LowerResult<_>>()?;
    Ok(VLIRStructuralBody { nets, submodules })
}

// ── Sequential body ─────────────────────────────────────────────────────────

fn lower_seq(s: &SHIRSeqBody, leg: &Legalizer, registered_outs: &HashSet<String>) -> LowerResult<VLIRSeqBody> {
    let clock = leg.get(&s.clock);

    let reg_decls = s
        .registers
        .iter()
        .map(|r| VLIRRegDecl { name: leg.get(&r.name), width: width_of(&r.ty) })
        .collect();

    let submodules = s.submodules.iter().map(|m| lower_submodule(m, leg)).collect::<LowerResult<_>>()?;

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

    // Memory nets: widths for the accesses to drive, and which ports are used at
    // all (an unused port of a multi-port memory gets no nets).
    let mem_widths = MemWidths::build(&s.memories);
    let mut mem_use = MemPortUse::default();
    for phase in &s.phases {
        collect_mem_use(&phase.pre_edge, &mut mem_use);
        // A read result observed after the tick lands in a register update, not
        // in the pre-edge statements — that is the whole point of `data()`.
        for u in &phase.post_edge {
            collect_mem_use_expr(&u.next_value, &mut mem_use);
        }
    }
    let memories = lower_mem_decls(&s.memories, &mem_use, leg);

    let mut comb_phases = Vec::new();
    let mut output_assigns = Vec::new();
    let mut ff_stmts = Vec::new();

    for phase in &s.phases {
        let (mut stmts, mut outs) = lower_flat_stmts(&phase.pre_edge, leg, &hold_outs, &mem_widths)?;
        // Memory net defaults go first, so a port driven only inside an `if` is
        // still assigned on every path (no latch, and the enable reads as 0).
        // A module using memory has exactly one phase, so phase 0 owns them.
        if phase.phase_idx == 0 {
            let defaults = mem_net_defaults(&s.memories, &mem_use, leg);
            stmts.splice(0..0, defaults);
        }
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
        hoist_branch_local_defaults(&mut stmts);
        check_no_latches(&stmts)?;
        comb_phases.push(VLIRCombPhase { phase_guard, stmts });

        // Register updates -> always_ff non-blocking assigns, guarded by phase
        // for multi-phase modules.
        let updates = lower_reg_updates(&phase.post_edge, leg)?;
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

    // Memory write commits close out `always_ff`; nonblocking assignment makes
    // their position relative to the register updates immaterial.
    ff_stmts.extend(mem_write_commits(&s.memories, &mem_use, leg));

    Ok(VLIRSeqBody {
        clock: clock.clone(),
        reg_decls,
        memories,
        submodules,
        comb_phases,
        always_ff: VLIRAlwaysFF { clock, stmts: ff_stmts },
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

fn lower_reg_updates(updates: &[SHIRRegUpdate], leg: &Legalizer) -> LowerResult<Vec<VLIRFFStmt>> {
    let mut out = Vec::new();
    for u in updates {
        let target = leg.get(&u.target);
        // Top-level case-as-expression is lifted into a case statement.
        if let SHIRExpr::Case { scrutinee, arms, default } = &u.next_value {
            let selector = lower_expr(scrutinee, leg)?;
            let mut ff_arms = Vec::new();
            for arm in arms {
                let selector_value = pattern_to_selector(&arm.pattern)?;
                ff_arms.push(copper_core::vlir::VLIRFFCaseArm {
                    selector_value,
                    stmts: vec![VLIRFFStmt::NonBlockingAssign {
                        target: target.clone(),
                        value: lower_expr(&arm.value, leg)?,
                    }],
                });
            }
            out.push(VLIRFFStmt::Case {
                selector,
                arms: ff_arms,
                default: Some(vec![VLIRFFStmt::NonBlockingAssign {
                    target: target.clone(),
                    value: lower_expr(default, leg)?,
                }]),
            });
        } else {
            out.push(VLIRFFStmt::NonBlockingAssign {
                target,
                value: lower_expr(&u.next_value, leg)?,
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
) -> LowerResult<(Vec<VLIRStmt>, Vec<VLIRContinuousAssign>)> {
    let mut comb = Vec::new();
    let mut assigns = Vec::new();
    for s in stmts {
        match s {
            SHIRStmt::Wire { name, ty, value } => comb.push(VLIRStmt::WireAssign {
                name: leg.get(name),
                width: width_of(ty),
                value: lower_expr(value, leg)?,
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
            SHIRStmt::PortDrive { port_name, value }
                if hold_outs.contains(&leg.get(port_name)) =>
            {
                comb.push(VLIRStmt::PortAssign {
                    port_name: leg.get(port_name),
                    value: lower_expr(value, leg)?,
                })
            }
            SHIRStmt::PortDrive { port_name, value } => assigns.push(VLIRContinuousAssign {
                target: leg.get(port_name),
                value: lower_expr(value, leg)?,
            }),
            // Conditional structures — including ones that drive output ports
            // (a Moore output). These lower into `always_comb`, where the port is
            // assigned on every path, so no latch is inferred.
            SHIRStmt::If { .. } | SHIRStmt::Match { .. } | SHIRStmt::ForLoop { .. }
            | SHIRStmt::IndexAssign { .. } => {
                comb.push(lower_comb_stmt(s, leg, mems)?);
            }
            // A memory access drives its port's nets; unconditionally here, or
            // under the enclosing `if` when it came from one.
            SHIRStmt::MemRead { .. } | SHIRStmt::MemWrite { .. } => {
                comb.extend(lower_mem_access(s, leg, mems)?);
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
) -> LowerResult<Vec<VLIRStmt>> {
    let mut out = Vec::new();
    for s in stmts {
        match s {
            SHIRStmt::MemRead { .. } | SHIRStmt::MemWrite { .. } => {
                out.extend(lower_mem_access(s, leg, mems)?)
            }
            _ => out.push(lower_comb_stmt(s, leg, mems)?),
        }
    }
    Ok(out)
}

/// The nets a single memory access drives, in `always_comb`.
fn lower_mem_access(
    s: &SHIRStmt,
    leg: &Legalizer,
    mems: &MemWidths,
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
    let addr_net = leg.get(&mem_net(mem, is_read, port, "addr"));
    let mut out = vec![
        VLIRStmt::WireAssign {
            name: leg.get(&mem_net(mem, is_read, port, "en")),
            width: one.clone(),
            value: VLIRExpr::Lit { width: one, value: 1 },
        },
        VLIRStmt::WireAssign {
            name: addr_net,
            width: mems.addr(mem),
            value: lower_expr(addr, leg)?,
        },
    ];
    if let Some(v) = value {
        out.push(VLIRStmt::WireAssign {
            name: leg.get(&mem_net(mem, is_read, port, "data")),
            width: mems.data(mem),
            value: lower_expr(v, leg)?,
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
) -> LowerResult<VLIRStmt> {
    match s {
        SHIRStmt::Wire { name, ty, value } => Ok(VLIRStmt::WireAssign {
            name: leg.get(name),
            width: width_of(ty),
            value: lower_expr(value, leg)?,
        }),
        // A port driven inside a conditional becomes a blocking assign in
        // `always_comb` (the port is a `logic` output, assigned on every path).
        SHIRStmt::PortDrive { port_name, value } => Ok(VLIRStmt::PortAssign {
            port_name: leg.get(port_name),
            value: lower_expr(value, leg)?,
        }),
        SHIRStmt::If { condition, then_stmts, else_stmts } => Ok(VLIRStmt::If {
            condition: lower_expr(condition, leg)?,
            then_stmts: lower_comb_stmts(then_stmts, leg, mems)?,
            else_stmts: match else_stmts {
                Some(e) => Some(lower_comb_stmts(e, leg, mems)?),
                None => None,
            },
        }),
        SHIRStmt::Match { scrutinee, arms } => {
            let selector = lower_expr(scrutinee, leg)?;
            let mut case_arms = Vec::new();
            let mut default = None;
            for arm in arms {
                let stmts = lower_comb_stmts(&arm.stmts, leg, mems)?;
                // A bare wildcard arm becomes the `default` case.
                if arm.patterns.len() == 1
                    && matches!(arm.patterns[0], copper_core::shir::SHIRPattern::Wildcard)
                    && arm.guard.is_none()
                {
                    default = Some(stmts);
                } else {
                    for p in &arm.patterns {
                        case_arms.push(VLIRCaseArm {
                            selector_value: pattern_to_selector(p)?,
                            stmts: stmts
                                .iter()
                                .map(|s| clone_comb_stmt(s))
                                .collect::<Vec<_>>(),
                        });
                    }
                }
            }
            Ok(VLIRStmt::Case { selector, arms: case_arms, default })
        }
        SHIRStmt::ForLoop { var, start, end, body } => Ok(VLIRStmt::ForLoop {
            var: var.clone(),
            start: lower_expr(start, leg)?,
            end: lower_expr(end, leg)?,
            body: lower_comb_stmts(body, leg, mems)?,
        }),
        SHIRStmt::IndexAssign { base, index, value } => Ok(VLIRStmt::IndexAssign {
            base: leg.get(base),
            index: lower_expr(index, leg)?,
            value: lower_expr(value, leg)?,
        }),
        // Handled by `lower_comb_stmts`, the only caller that sees a list.
        SHIRStmt::MemRead { .. } | SHIRStmt::MemWrite { .. } => {
            Err(VLIRLowerError::MemoryAccessOutOfLine)
        }
    }
}

fn clone_comb_stmt(s: &VLIRStmt) -> VLIRStmt {
    match s {
        VLIRStmt::WireAssign { name, width, value } => VLIRStmt::WireAssign {
            name: name.clone(),
            width: width.clone(),
            value: value.clone(),
        },
        VLIRStmt::PortAssign { port_name, value } => VLIRStmt::PortAssign {
            port_name: port_name.clone(),
            value: value.clone(),
        },
        VLIRStmt::If { condition, then_stmts, else_stmts } => VLIRStmt::If {
            condition: condition.clone(),
            then_stmts: then_stmts.iter().map(clone_comb_stmt).collect(),
            else_stmts: else_stmts
                .as_ref()
                .map(|e| e.iter().map(clone_comb_stmt).collect()),
        },
        VLIRStmt::Case { selector, arms, default } => VLIRStmt::Case {
            selector: selector.clone(),
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

fn lower_submodule(m: &SHIRSubmoduleInst, leg: &Legalizer) -> LowerResult<VLIRSubmoduleInst> {
    Ok(VLIRSubmoduleInst {
        inst_name: leg.get(&m.inst_name),
        module_name: leg.get(&m.module_name),
        inputs: m
            .inputs
            .iter()
            .map(|(port, e)| Ok((leg.get(port), lower_expr(e, leg)?)))
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

fn lower_expr(e: &SHIRExpr, leg: &Legalizer) -> LowerResult<VLIRExpr> {
    Ok(match e {
        SHIRExpr::Var(name) => VLIRExpr::Var(leg.get(name)),
        SHIRExpr::Lit(lit) => lower_lit(lit),
        // The read port's output net, and its valid flag — which at a one-cycle
        // read latency is exactly "an address was staged for the edge that just
        // passed", i.e. the enable net itself.
        SHIRExpr::MemData { mem, port } => {
            VLIRExpr::Var(leg.get(&mem_net(mem, true, *port, "data")))
        }
        SHIRExpr::MemValid { mem, port } => {
            VLIRExpr::Var(leg.get(&mem_net(mem, true, *port, "en")))
        }
        SHIRExpr::BinOp { left, op, right } => VLIRExpr::BinOp {
            left: Box::new(lower_expr(left, leg)?),
            op: lower_binop(op),
            right: Box::new(lower_expr(right, leg)?),
        },
        SHIRExpr::UnOp { op, expr } => VLIRExpr::UnOp {
            op: lower_unop(op),
            expr: Box::new(lower_expr(expr, leg)?),
        },
        SHIRExpr::Mux { cond, then_val, else_val } => VLIRExpr::Ternary {
            cond: Box::new(lower_expr(cond, leg)?),
            then_val: Box::new(lower_expr(then_val, leg)?),
            else_val: Box::new(lower_expr(else_val, leg)?),
        },
        SHIRExpr::Concat(parts) => {
            VLIRExpr::Concat(parts.iter().map(|p| lower_expr(p, leg)).collect::<LowerResult<_>>()?)
        }
        SHIRExpr::Slice { expr, high, low } => VLIRExpr::Slice {
            expr: Box::new(lower_expr(expr, leg)?),
            high: *high,
            low: *low,
        },
        SHIRExpr::DynBit { base, index } => VLIRExpr::DynBit {
            base: Box::new(lower_expr(base, leg)?),
            index: Box::new(lower_expr(index, leg)?),
        },
        SHIRExpr::Resize { expr, width } => VLIRExpr::Resize {
            expr: Box::new(lower_expr(expr, leg)?),
            width: width.clone(),
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
            let sel = lower_expr(scrutinee, leg)?;
            let mut result = lower_expr(default, leg)?;
            // Fold from the last arm backwards so earlier arms take priority.
            for arm in arms.iter().rev() {
                let mut cond = VLIRExpr::BinOp {
                    left: Box::new(sel.clone()),
                    op: VLIRBinOp::Eq,
                    right: Box::new(pattern_to_selector(&arm.pattern)?),
                };
                if let Some(g) = &arm.guard {
                    cond = VLIRExpr::BinOp {
                        left: Box::new(cond),
                        op: VLIRBinOp::LogicalAnd,
                        right: Box::new(lower_expr(g, leg)?),
                    };
                }
                result = VLIRExpr::Ternary {
                    cond: Box::new(cond),
                    then_val: Box::new(lower_expr(&arm.value, leg)?),
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

/// Convert a scalar SHIR pattern into a concrete case-selector literal.
/// Tuple patterns are deferred to M2.
fn pattern_to_selector(p: &copper_core::shir::SHIRPattern) -> LowerResult<VLIRExpr> {
    use copper_core::shir::SHIRPattern;
    match p {
        SHIRPattern::Lit(lit) => Ok(lower_lit(lit)),
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
        SHIRPattern::Wildcard | SHIRPattern::EnumVariant { .. } => {
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
fn shir_expr_vars(e: &SHIRExpr, out: &mut HashSet<String>) {
    match e {
        SHIRExpr::Var(n) => {
            out.insert(n.clone());
        }
        SHIRExpr::BinOp { left, right, .. } => {
            shir_expr_vars(left, out);
            shir_expr_vars(right, out);
        }
        SHIRExpr::UnOp { expr, .. } | SHIRExpr::Slice { expr, .. } | SHIRExpr::Resize { expr, .. } => {
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
        // not signals the phase analysis can move or promote.
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
            SHIRStmt::PortDrive { port_name, value } => {
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
        let common = common.unwrap_or_default();
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
        stmts.insert(0, VLIRStmt::PortAssign { port_name, value });
    }
}

/// The value assigned to `port` by the first arm that drives it with a direct,
/// top-level `PortAssign` (not a nested/conditional drive).
fn first_direct_port_value(arms: &[VLIRCaseArm], port: &str) -> Option<VLIRExpr> {
    for a in arms {
        for s in &a.stmts {
            if let VLIRStmt::PortAssign { port_name, value } = s {
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
        VLIRStmt::PortAssign { port_name, value } if targets.contains(port_name) => {
            (None, vec![VLIRFFStmt::NonBlockingAssign { target: port_name.clone(), value: value.clone() }])
        }
        VLIRStmt::If { condition, then_stmts, else_stmts } => {
            let (tc, tf) = split_output_regs(then_stmts, targets);
            let (ec, ef) = match else_stmts {
                Some(e) => { let (c, f) = split_output_regs(e, targets); (Some(c), f) }
                None => (None, Vec::new()),
            };
            let comb = if tc.is_empty() && ec.as_ref().map_or(true, |c| c.is_empty()) {
                None
            } else {
                Some(VLIRStmt::If { condition: condition.clone(), then_stmts: tc, else_stmts: ec })
            };
            let ff = if tf.is_empty() && ef.is_empty() {
                Vec::new()
            } else {
                vec![VLIRFFStmt::If {
                    condition: condition.clone(),
                    then_stmts: tf,
                    else_stmts: if ef.is_empty() { None } else { Some(ef) },
                }]
            };
            (comb, ff)
        }
        VLIRStmt::Case { selector, arms, default } => {
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
                Some(VLIRStmt::Case { selector: selector.clone(), arms: comb_arms, default: comb_default })
            };
            let ff = if ff_arms.is_empty() && ff_default.is_none() {
                Vec::new()
            } else {
                // A registered hold needs a complete case: the empty default means
                // "no assignment → the output register holds" (and avoids a
                // Verilator CASEINCOMPLETE warning).
                vec![VLIRFFStmt::Case {
                    selector: selector.clone(),
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
fn hoist_branch_local_defaults(stmts: &mut Vec<VLIRStmt>) {
    let top_assigned: HashSet<String> = stmts.iter().filter_map(top_level_assign_name).collect();
    let mut hoisted: Vec<VLIRStmt> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    strip_nested_defaults(stmts, false, &top_assigned, &mut hoisted, &mut seen);
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
    hoisted: &mut Vec<VLIRStmt>,
    seen: &mut HashSet<String>,
) {
    let mut keep = Vec::with_capacity(body.len());
    for mut s in body.drain(..) {
        // Recurse into sub-bodies first (they are one level more nested).
        match &mut s {
            VLIRStmt::If { then_stmts, else_stmts, .. } => {
                strip_nested_defaults(then_stmts, true, top_assigned, hoisted, seen);
                if let Some(e) = else_stmts {
                    strip_nested_defaults(e, true, top_assigned, hoisted, seen);
                }
            }
            VLIRStmt::Case { arms, default, .. } => {
                for a in arms {
                    strip_nested_defaults(&mut a.stmts, true, top_assigned, hoisted, seen);
                }
                if let Some(d) = default {
                    strip_nested_defaults(d, true, top_assigned, hoisted, seen);
                }
            }
            VLIRStmt::ForLoop { body, .. } => {
                strip_nested_defaults(body, true, top_assigned, hoisted, seen);
            }
            _ => {}
        }
        // A nested literal-init WireAssign to a not-top-assigned name is a
        // block-local default: hoist it (once) and strip it from the branch.
        let hoistable = if is_nested {
            match &s {
                VLIRStmt::WireAssign { name, value: VLIRExpr::Lit { .. }, .. }
                    if !top_assigned.contains(name) =>
                {
                    Some(name.clone())
                }
                _ => None,
            }
        } else {
            None
        };
        if let Some(name) = hoistable {
            if seen.insert(name) {
                hoisted.push(s);
            }
            continue; // stripped from the branch either way
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
struct MemWidths(HashMap<String, (Width, Width)>);

impl MemWidths {
    fn build(memories: &[SHIRMemory]) -> Self {
        MemWidths(
            memories
                .iter()
                .map(|m| (m.name.clone(), (addr_width(m.depth), width_of(&m.elem_ty))))
                .collect(),
        )
    }
    fn addr(&self, mem: &str) -> Width {
        self.0.get(mem).map(|(a, _)| a.clone()).unwrap_or(Width::Concrete(1))
    }
    fn data(&self, mem: &str) -> Width {
        self.0.get(mem).map(|(_, d)| d.clone()).unwrap_or(Width::Concrete(1))
    }
}

fn mem_net(mem: &str, is_read: bool, port: usize, suffix: &str) -> String {
    format!("{mem}_{}{port}_{suffix}", if is_read { "rd" } else { "wr" })
}

/// Bits needed to index `depth` entries (at least one).
fn addr_width(depth: usize) -> Width {
    let bits = if depth <= 1 { 1 } else { (usize::BITS - (depth - 1).leading_zeros()) as usize };
    Width::Concrete(bits)
}

/// Which memory ports a body actually touches. Only these get nets, so an
/// unused port of a multi-port memory does not leave dangling signals behind.
#[derive(Default)]
struct MemPortUse {
    /// `(mem, port)` staged by a `read()` call.
    reads: HashSet<(String, usize)>,
    /// `(mem, port)` staged by a `write()` call.
    writes: HashSet<(String, usize)>,
    /// `(mem, port)` whose `data()` output is observed.
    read_data: HashSet<(String, usize)>,
    /// `(mem, port)` whose `is_ready()` flag is observed.
    read_valid: HashSet<(String, usize)>,
}

impl MemPortUse {
    /// A read port needs its enable/address nets if it is staged *or* merely
    /// observed: a `data()` read with no `read()` anywhere is a port that never
    /// becomes ready, which the emitted `en = 1'b0` default reproduces exactly.
    /// Without this the observation would reference nets nothing declares.
    fn read_touched(&self, mem: &str, port: usize) -> bool {
        let key = (mem.to_string(), port);
        self.reads.contains(&key)
            || self.read_data.contains(&key)
            || self.read_valid.contains(&key)
    }
}

fn collect_mem_use(stmts: &[SHIRStmt], use_: &mut MemPortUse) {
    for s in stmts {
        match s {
            SHIRStmt::MemRead { mem, port, addr } => {
                use_.reads.insert((mem.clone(), *port));
                collect_mem_use_expr(addr, use_);
            }
            SHIRStmt::MemWrite { mem, port, addr, value } => {
                use_.writes.insert((mem.clone(), *port));
                collect_mem_use_expr(addr, use_);
                collect_mem_use_expr(value, use_);
            }
            SHIRStmt::Wire { value, .. } | SHIRStmt::PortDrive { value, .. } => {
                collect_mem_use_expr(value, use_)
            }
            SHIRStmt::If { condition, then_stmts, else_stmts } => {
                collect_mem_use_expr(condition, use_);
                collect_mem_use(then_stmts, use_);
                if let Some(e) = else_stmts {
                    collect_mem_use(e, use_);
                }
            }
            SHIRStmt::Match { scrutinee, arms } => {
                collect_mem_use_expr(scrutinee, use_);
                for a in arms {
                    if let Some(g) = &a.guard {
                        collect_mem_use_expr(g, use_);
                    }
                    collect_mem_use(&a.stmts, use_);
                }
            }
            SHIRStmt::ForLoop { start, end, body, .. } => {
                collect_mem_use_expr(start, use_);
                collect_mem_use_expr(end, use_);
                collect_mem_use(body, use_);
            }
            SHIRStmt::IndexAssign { index, value, .. } => {
                collect_mem_use_expr(index, use_);
                collect_mem_use_expr(value, use_);
            }
        }
    }
}

fn collect_mem_use_expr(e: &SHIRExpr, use_: &mut MemPortUse) {
    match e {
        SHIRExpr::MemData { mem, port } => {
            use_.read_data.insert((mem.clone(), *port));
        }
        // `is_ready()` reads the enable net.
        SHIRExpr::MemValid { mem, port } => {
            use_.read_valid.insert((mem.clone(), *port));
        }
        SHIRExpr::Var(_) | SHIRExpr::Lit(_) | SHIRExpr::PhaseEq(_) => {}
        SHIRExpr::BinOp { left, right, .. } => {
            collect_mem_use_expr(left, use_);
            collect_mem_use_expr(right, use_);
        }
        SHIRExpr::UnOp { expr, .. } | SHIRExpr::Resize { expr, .. } | SHIRExpr::Slice { expr, .. } => {
            collect_mem_use_expr(expr, use_)
        }
        SHIRExpr::Mux { cond, then_val, else_val } => {
            collect_mem_use_expr(cond, use_);
            collect_mem_use_expr(then_val, use_);
            collect_mem_use_expr(else_val, use_);
        }
        SHIRExpr::Case { scrutinee, arms, default } => {
            collect_mem_use_expr(scrutinee, use_);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_mem_use_expr(g, use_);
                }
                collect_mem_use_expr(&a.value, use_);
            }
            collect_mem_use_expr(default, use_);
        }
        SHIRExpr::Concat(parts) => {
            for p in parts {
                collect_mem_use_expr(p, use_);
            }
        }
        SHIRExpr::DynBit { base, index } => {
            collect_mem_use_expr(base, use_);
            collect_mem_use_expr(index, use_);
        }
    }
}

/// Top-of-`always_comb` defaults for every accessed memory net. Without these an
/// enable driven only inside an `if` would infer a latch (and `check_no_latches`
/// would reject the module).
fn mem_net_defaults(
    memories: &[SHIRMemory],
    use_: &MemPortUse,
    leg: &Legalizer,
) -> Vec<VLIRStmt> {
    let mut out = Vec::new();
    let one_bit = Width::Concrete(1);
    for m in memories {
        let aw = addr_width(m.depth);
        let dw = width_of(&m.elem_ty);
        let mut push = |is_read: bool, port: usize, with_data: bool| {
            for (suffix, width) in [("en", one_bit.clone()), ("addr", aw.clone())]
                .into_iter()
                .chain(with_data.then(|| ("data", dw.clone())))
            {
                out.push(VLIRStmt::WireAssign {
                    name: leg.get(&mem_net(&m.name, is_read, port, suffix)),
                    width: width.clone(),
                    value: VLIRExpr::Lit { width, value: 0 },
                });
            }
        };
        for port in 0..m.read_ports {
            if use_.read_touched(&m.name, port) {
                push(true, port, false);
            }
        }
        for port in 0..m.write_ports {
            if use_.writes.contains(&(m.name.clone(), port)) {
                push(false, port, true);
            }
        }
    }
    out
}

/// The array declarations plus the continuous read-data nets.
fn lower_mem_decls(memories: &[SHIRMemory], use_: &MemPortUse, leg: &Legalizer) -> Vec<VLIRMemDecl> {
    memories
        .iter()
        .map(|m| VLIRMemDecl {
            name: leg.get(&m.name),
            width: width_of(&m.elem_ty),
            depth: m.depth,
            read_data_nets: (0..m.read_ports)
                .filter(|p| use_.read_data.contains(&(m.name.clone(), *p)))
                .map(|p| VLIRMemReadNet {
                    data: leg.get(&mem_net(&m.name, true, p, "data")),
                    addr: leg.get(&mem_net(&m.name, true, p, "addr")),
                    width: width_of(&m.elem_ty),
                })
                .collect(),
        })
        .collect()
}

/// `if (<mem>_wr<J>_en) <mem>[<mem>_wr<J>_addr] <= <mem>_wr<J>_data;` — one per
/// staged write port. The commit is unguarded by phase because a module using
/// memory has exactly one phase (enforced in `chir_lower`).
fn mem_write_commits(memories: &[SHIRMemory], use_: &MemPortUse, leg: &Legalizer) -> Vec<VLIRFFStmt> {
    let mut out = Vec::new();
    for m in memories {
        for port in 0..m.write_ports {
            if !use_.writes.contains(&(m.name.clone(), port)) {
                continue;
            }
            out.push(VLIRFFStmt::If {
                condition: VLIRExpr::Var(leg.get(&mem_net(&m.name, false, port, "en"))),
                then_stmts: vec![VLIRFFStmt::MemAssign {
                    mem: leg.get(&m.name),
                    addr: VLIRExpr::Var(leg.get(&mem_net(&m.name, false, port, "addr"))),
                    value: VLIRExpr::Var(leg.get(&mem_net(&m.name, false, port, "data"))),
                }],
                else_stmts: None,
            });
        }
    }
    out
}

// ── Width helper ────────────────────────────────────────────────────────────

fn width_of(ty: &CHIRType) -> Width {
    match ty {
        CHIRType::UInt { width } | CHIRType::SInt { width } => width.clone(),
        CHIRType::Bool => Width::Concrete(1),
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

/// Verilog / SystemVerilog reserved keywords (VLIR_DESIGN §Pass 1).
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
    ];
    KEYWORDS.contains(&name)
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

        // (State=2 :: 3 bits, in=0 :: 1 bit) -> {010, 0} = 4'd4
        let p = SHIRPattern::Tuple(vec![lit(3, 2), lit(1, 0)]);
        match pattern_to_selector(&p).expect("selector") {
            VLIRExpr::Lit { width, value } => {
                assert_eq!(width, Width::Concrete(4));
                assert_eq!(value, 4);
            }
            other => panic!("expected literal selector, got {other:?}"),
        }

        // (State=5 :: 3 bits, in=1 :: 1 bit) -> {101, 1} = 4'd11
        let p = SHIRPattern::Tuple(vec![lit(3, 5), lit(1, 1)]);
        match pattern_to_selector(&p).expect("selector") {
            VLIRExpr::Lit { value, .. } => assert_eq!(value, 11),
            other => panic!("expected literal selector, got {other:?}"),
        }

        // A wildcard inside a tuple has no single selector value → rejected.
        let p = SHIRPattern::Tuple(vec![lit(3, 1), SHIRPattern::Wildcard]);
        assert!(pattern_to_selector(&p).is_err());
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
