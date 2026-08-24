use std::collections::{HashMap, HashSet};

use copper_core::chir::{
    CHIRBody, CHIRExpr, CHIRLit, CHIRMemInit, CHIRModule, CHIRPattern, CHIRPort, CHIRPortDir,
    CHIRPortKind, CHIRRegDecl, CHIRSeqBody, CHIRStmt, CHIRStructuralBody, CHIRSubmoduleInst,
    CHIRType, Width,
};
use copper_core::frontend_ir::SourceSpan;
use copper_core::shir::{
    SHIRBody, SHIRCaseArm, SHIRCombBody, SHIRExpr, SHIRLit, SHIRLowerError, SHIRMatchArm,
    SHIRMemInit,
    SHIRMemory, SHIRModule, SHIRPattern, SHIRPhase, SHIRPort,
    SHIRPortDir, SHIRPortKind, SHIRReg, SHIRRegUpdate, SHIRSeqBody, SHIRStmt,
    SHIRStructuralBody, SHIRSubmoduleInst,
};

// ── Public entry point ────────────────────────────────────────────────────────

pub fn lower_to_shir(chir: &CHIRModule) -> Result<SHIRModule, SHIRLowerError> {
    let ports = lower_ports(&chir.ports);

    let body = match &chir.body {
        CHIRBody::Combinational(comb) => {
            SHIRBody::Combinational(lower_comb_body(comb)?)
        }
        CHIRBody::Sequential(seq) => {
            SHIRBody::Sequential(lower_seq_body(seq, chir.span)?)
        }
        CHIRBody::Structural(st) => {
            SHIRBody::Structural(lower_structural_body(st)?)
        }
    };

    Ok(SHIRModule {
        name: chir.name.clone(),
        params: chir.params.clone(),
        ports,
        body,
        span: chir.span,
    })
}

// ── Port lowering ─────────────────────────────────────────────────────────────

fn lower_ports(ports: &[CHIRPort]) -> Vec<SHIRPort> {
    ports.iter().map(|p| SHIRPort {
        name: p.name.clone(),
        direction: match p.direction {
            CHIRPortDir::Input => SHIRPortDir::Input,
            CHIRPortDir::Output => SHIRPortDir::Output,
        },
        kind: match &p.kind {
            CHIRPortKind::Clock { .. } => SHIRPortKind::Clock,
            CHIRPortKind::Data { ty } => SHIRPortKind::Data { ty: ty.clone() },
        },
        registered: p.registered,
        span: p.span,
    }).collect()
}

// ── Combinational body ────────────────────────────────────────────────────────

fn lower_comb_body(
    comb: &copper_core::chir::CHIRCombBody,
) -> Result<SHIRCombBody, SHIRLowerError> {
    let submodules = comb.submodules.iter()
        .map(lower_submodule)
        .collect::<Result<_, _>>()?;

    // A combinational local may be *reassigned* (`acc = acc + 1`, incl. inside a
    // loop). Such an Assign becomes another blocking assign to the same signal;
    // its declared type comes from the original `let` wire, collected here.
    let mut wire_types = HashMap::new();
    collect_wire_types(&comb.stmts, &mut wire_types);

    let stmts = lower_stmt_list(&comb.stmts, &std::collections::HashSet::new(), &HashMap::new(), &wire_types)?;

    Ok(SHIRCombBody { submodules, stmts })
}

/// Collect every combinational wire's declared type (`let` bindings), recursing
/// into conditional and loop bodies.
fn collect_wire_types(
    stmts: &[CHIRStmt],
    out: &mut HashMap<String, copper_core::chir::CHIRType>,
) {
    for stmt in stmts {
        match stmt {
            CHIRStmt::Wire { name, ty, .. } => { out.insert(name.clone(), ty.clone()); }
            CHIRStmt::If { then_body, else_body, .. } => {
                collect_wire_types(then_body, out);
                if let Some(eb) = else_body { collect_wire_types(eb, out); }
            }
            CHIRStmt::Match { arms, .. } => {
                for a in arms { collect_wire_types(&a.body, out); }
            }
            CHIRStmt::ForLoop { body, .. } => collect_wire_types(body, out),
            _ => {}
        }
    }
}

/// Lower a CHIR statement list into SHIR statements, handling wire promotion renames.
/// Used in both comb body lowering and pre_edge lowering. `wire_types` names the
/// combinational wires that a reassignment may target (empty for the sequential
/// pre-edge path, where an `Assign` is a register update handled elsewhere).
fn lower_stmt_list(
    stmts: &[CHIRStmt],
    promoted_names: &std::collections::HashSet<String>,
    renames: &HashMap<String, String>,
    wire_types: &HashMap<String, copper_core::chir::CHIRType>,
) -> Result<Vec<SHIRStmt>, SHIRLowerError> {
    let mut out = Vec::new();
    for stmt in stmts {
        match stmt {
            // A reassignment of a known combinational wire → another blocking
            // assign to it (`name = value;`). The single `logic` declaration is
            // deduplicated downstream, so repeated assigns share one wire.
            CHIRStmt::Assign { target, value, .. } if wire_types.contains_key(target) => {
                out.push(SHIRStmt::Wire {
                    name: target.clone(),
                    ty: wire_types[target].clone(),
                    value: rename_vars(lower_expr(value)?, renames),
                });
            }
            CHIRStmt::Wire { name, ty, value, .. } => {
                out.push(SHIRStmt::Wire {
                    name: name.clone(),
                    ty: ty.clone(),
                    value: rename_vars(lower_expr(value)?, renames),
                });
            }
            CHIRStmt::PortWrite { port_name, value, .. } => {
                out.push(SHIRStmt::PortDrive {
                    port_name: port_name.clone(),
                    value: rename_vars(lower_expr(value)?, renames),
                });
            }
            CHIRStmt::If { condition, then_body, else_body, .. } => {
                let then_stmts = lower_stmt_list(then_body, promoted_names, renames, wire_types)?;
                let else_stmts = else_body.as_ref()
                    .map(|eb| lower_stmt_list(eb, promoted_names, renames, wire_types))
                    .transpose()?;
                if !then_stmts.is_empty() || else_stmts.as_ref().map_or(false, |e| !e.is_empty()) {
                    out.push(SHIRStmt::If {
                        condition: rename_vars(lower_expr(condition)?, renames),
                        then_stmts,
                        else_stmts,
                    });
                }
            }
            CHIRStmt::Match { scrutinee, arms, .. } => {
                let shir_arms = arms.iter()
                    .map(|arm| {
                        let stmts = lower_stmt_list(&arm.body, promoted_names, renames, wire_types)?;
                        Ok(SHIRMatchArm {
                            patterns: arm.patterns.iter()
                                .map(|p| lower_pattern(p))
                                .collect::<Result<_, _>>()?,
                            guard: arm.guard.as_ref()
                                .map(|g| Ok(rename_vars(lower_expr(g)?, renames)))
                                .transpose()?,
                            stmts,
                        })
                    })
                    .collect::<Result<Vec<_>, SHIRLowerError>>()?;
                if shir_arms.iter().any(|a| !a.stmts.is_empty()) {
                    out.push(SHIRStmt::Match {
                        scrutinee: rename_vars(lower_expr(scrutinee)?, renames),
                        arms: shir_arms,
                    });
                }
            }
            CHIRStmt::ForLoop { var, start, end, body, .. } => {
                let body_stmts = lower_stmt_list(body, promoted_names, renames, wire_types)?;
                if !body_stmts.is_empty() {
                    out.push(SHIRStmt::ForLoop {
                        var: var.clone(),
                        start: rename_vars(lower_expr(start)?, renames),
                        end: rename_vars(lower_expr(end)?, renames),
                        body: body_stmts,
                    });
                }
            }
            CHIRStmt::IndexAssign { base, index, value, .. } => {
                out.push(SHIRStmt::IndexAssign {
                    base: base.clone(),
                    index: rename_vars(lower_expr(index)?, renames),
                    value: rename_vars(lower_expr(value)?, renames),
                });
            }
            // Memory accesses stage the address/data buses that this segment's
            // clock edge captures. They ride along with the pre-edge statements so
            // their surrounding `if` structure (the port enable) is preserved.
            CHIRStmt::MemRead { mem, port, addr, .. } => {
                out.push(SHIRStmt::MemRead {
                    mem: mem.clone(),
                    port: *port,
                    addr: rename_vars(lower_expr(addr)?, renames),
                });
            }
            CHIRStmt::MemWrite { mem, port, addr, value, .. } => {
                out.push(SHIRStmt::MemWrite {
                    mem: mem.clone(),
                    port: *port,
                    addr: rename_vars(lower_expr(addr)?, renames),
                    value: rename_vars(lower_expr(value)?, renames),
                });
            }
            // Assign, AwaitTick — not pre_edge statements
            _ => {}
        }
    }
    Ok(out)
}

fn lower_submodule(sub: &CHIRSubmoduleInst) -> Result<SHIRSubmoduleInst, SHIRLowerError> {
    let inputs = sub.inputs.iter()
        .map(|(name, expr)| Ok((name.clone(), lower_expr(expr)?)))
        .collect::<Result<Vec<_>, SHIRLowerError>>()?;
    Ok(SHIRSubmoduleInst {
        inst_name: sub.inst_name.clone(),
        module_name: sub.module_name.clone(),
        inputs,
        output_wire: sub.output_wire.clone(),
        output_ty: sub.output_ty.clone(),
        clocks: sub.clocks.clone(),
        port_nets: sub.port_nets.clone(),
        output_port: sub.output_port.clone(),
    })
}

/// Structural body → SHIR. No timing regions to derive; nets and submodule
/// instances pass through 1:1.
fn lower_structural_body(st: &CHIRStructuralBody) -> Result<SHIRStructuralBody, SHIRLowerError> {
    let submodules = st.submodules.iter().map(lower_submodule).collect::<Result<Vec<_>, _>>()?;
    Ok(SHIRStructuralBody {
        nets: st.nets.clone(),
        submodules,
    })
}

// ── Sequential body ───────────────────────────────────────────────────────────

fn lower_seq_body(
    seq: &CHIRSeqBody,
    module_span: SourceSpan,
) -> Result<SHIRSeqBody, SHIRLowerError> {
    // Step 1: Validate CHIR preconditions
    validate_seq_chir(seq, module_span)?;

    // Step 2: Split loop_body into segments at AwaitTick boundaries
    let segments = split_at_ticks(&seq.loop_body);
    let n_ticks = segments.len() - 1; // N ticks → N+1 segments (0..=N)

    if n_ticks == 0 {
        return Err(SHIRLowerError::NoTick { span: module_span });
    }

    // Step 3: Build wire-to-segment map for register promotion analysis
    // Maps wire name → segment index where it was declared
    let wire_segment: HashMap<String, usize> = segments.iter().enumerate()
        .flat_map(|(seg_idx, stmts)| {
            stmts.iter().filter_map(move |s| {
                if let CHIRStmt::Wire { name, .. } = s {
                    Some((name.clone(), seg_idx))
                } else {
                    None
                }
            })
        })
        .collect();

    // Step 7: Find wires that need promotion (used in a different phase than declared)
    let promoted_wires = find_promoted_wires(&segments, &wire_segment, n_ticks);

    // Step 8: Lower registers (from CHIR register declarations)
    let mut registers: Vec<SHIRReg> = seq.registers.iter()
        .map(|r| lower_reg_decl(r))
        .collect::<Result<_, _>>()?;

    // Add promoted wire registers
    for (wire_name, wire_ty, wire_init) in &promoted_wires {
        registers.push(SHIRReg {
            name: format!("{}_r", wire_name),
            ty: wire_ty.clone(),
            init: wire_init.clone(),
        });
    }

    let promoted_names: HashSet<String> = promoted_wires.iter()
        .map(|(name, _, _)| name.clone())
        .collect();

    // Step 9: Lower submodules
    let submodules = seq.submodules.iter()
        .map(lower_submodule)
        .collect::<Result<_, _>>()?;

    // Step 10: Build phases
    let mut phases: Vec<SHIRPhase> = Vec::new();

    // Single-tick: no cross-phase promoted wires, so renames is always empty.
    let no_renames: HashMap<String, String> = HashMap::new();

    if n_ticks == 1 {
        // ── Single-tick: one phase.
        // pre_edge = wires from seg_0 (before tick).
        // post_edge = reg_assigns from seg_0 (captured at clock edge) +
        //             reg_assigns from seg_1 / trailing (same edge, next iteration wraps here).
        // Both segments share a forwarding map so seg_1 sees seg_0's register assignments.
        let mut pre_edge = lower_pre_edge_stmts(&segments[0], &promoted_names, &no_renames)?;
        // The trailing (post-tick) segment's *combinational* logic — wires and
        // output-port drives, including inside `if`/`match` — belongs to this
        // phase's pre-edge: it computes the registered outputs from the state the
        // edge just latched (e.g. a Moore output written after the tick). Passing
        // the whole segment through the pre-edge lowering keeps those and drops
        // the register reassignments (which `extract_reg_updates` below turns into
        // this edge's post_edge updates). Previously only `Wire`s were hoisted, so
        // a post-tick `out.write(...)` was silently dropped.
        pre_edge.extend(lower_pre_edge_stmts(&segments[1], &promoted_names, &no_renames)?);
        let mut all_post_edge = extract_reg_updates(
            &segments[0],
            &seq.registers,
            &promoted_names,
            module_span,
            &no_renames,
        )?;
        let trailing_updates = extract_reg_updates(
            &segments[1],
            &seq.registers,
            &promoted_names,
            module_span,
            &no_renames,
        )?;
        all_post_edge.extend(trailing_updates);

        phases.push(SHIRPhase {
            phase_idx: 0,
            pre_edge,
            post_edge: all_post_edge,
        });
    } else {
        // ── Multi-tick: N phases
        //
        // Mapping:
        //   seg_k (for k < n_ticks) → phase_k pre_edge + post_edge
        //   seg_n_ticks (trailing)  → phase_{n_ticks-1} post_edge
        //
        // Phase renames: promoted wires declared in earlier phases are renamed
        // from `x` to `x_r` in the phase where they are consumed.

        for phase_idx in 0..n_ticks {
            let seg_idx = phase_idx; // seg_k → phase_k

            // Build rename map: promoted wires from phases < phase_idx
            let phase_renames: HashMap<String, String> = promoted_wires.iter()
                .filter(|(wire_name, _, _)| {
                    let decl_seg = *wire_segment.get(wire_name.as_str()).unwrap();
                    phase_for_segment(decl_seg, n_ticks) < phase_idx
                })
                .map(|(wire_name, _, _)| (wire_name.clone(), format!("{}_r", wire_name)))
                .collect();

            let pre_edge = lower_pre_edge_stmts(&segments[seg_idx], &promoted_names, &phase_renames)?;

            let mut post_edge = extract_reg_updates(
                &segments[seg_idx],
                &seq.registers,
                &promoted_names,
                module_span,
                &phase_renames,
            )?;

            if phase_idx == n_ticks - 1 {
                // The trailing segment's REGISTER updates map to this phase. Its
                // combinational statements have no home in the multi-tick model
                // (only the single-tick path hoists them, where the trailing
                // segment shares the one phase) and used to be dropped SILENTLY —
                // an output written after the last tick simply vanished, leaving
                // an undriven port. Fail loudly instead; deciding which phase they
                // belong to is a semantics question, not a lowering detail.
                let trailing_comb =
                    lower_pre_edge_stmts(&segments[n_ticks], &promoted_names, &phase_renames)?;
                if !trailing_comb.is_empty() {
                    return Err(SHIRLowerError::UnsupportedConstruct {
                        description: "combinational statements after the last `clk.tick().await`                                       of a multi-tick loop are not supported (an output written                                       there would be silently dropped); move them before the last                                       tick"
                            .to_string(),
                        span: module_span,
                    });
                }
                // Trailing segment maps to the same phase — same renames apply
                let trailing_updates = extract_reg_updates(
                    &segments[n_ticks],
                    &seq.registers,
                    &promoted_names,
                    module_span,
                    &phase_renames,
                )?;
                post_edge.extend(trailing_updates);
            }

            // Capture promoted wires into their registers at this phase's clock edge
            for (wire_name, _ty, _init) in &promoted_wires {
                let decl_seg = wire_segment[wire_name.as_str()];
                if phase_for_segment(decl_seg, n_ticks) == phase_idx {
                    post_edge.push(SHIRRegUpdate {
                        target: format!("{}_r", wire_name),
                        next_value: SHIRExpr::Var(wire_name.clone()),
                    });
                }
            }

            // Phase advance (wraps back to 0 from N-1)
            let next_phase = (phase_idx + 1) % n_ticks;
            let phase_width = phase_register_width(n_ticks);
            post_edge.push(SHIRRegUpdate {
                target: "phase_r".to_string(),
                next_value: SHIRExpr::Lit(SHIRLit {
                    ty: CHIRType::UInt { width: Width::Concrete(phase_width) },
                    value: next_phase as u128,
                }),
            });

            phases.push(SHIRPhase { phase_idx, pre_edge, post_edge });
        }

        // Add phase_r register
        let phase_width = phase_register_width(n_ticks);
        registers.push(SHIRReg {
            name: "phase_r".to_string(),
            ty: CHIRType::UInt { width: Width::Concrete(phase_width) },
            init: Some(SHIRLit { ty: CHIRType::UInt { width: Width::Concrete(phase_width) }, value: 0 }),
        });
    }

    Ok(SHIRSeqBody {
        clock: seq.clock.clone(),
        registers,
        memories: seq.memories.iter().map(|m| Ok(SHIRMemory {
            name: m.name.clone(),
            elem_ty: m.elem_ty.clone(),
            depth: m.depth,
            read_ports: m.read_ports,
            write_ports: m.write_ports,
            read_lat: m.read_lat,
            write_lat: m.write_lat,
            init: m.init.as_ref().map(lower_mem_init).transpose()?,
            write_mode: m.write_mode,
        })).collect::<Result<Vec<_>, SHIRLowerError>>()?,
        submodules,
        phases,
    })
}

// ── Helpers: segment splitting ────────────────────────────────────────────────

/// Split `loop_body` into segments at each `AwaitTick` boundary.
///
/// N `AwaitTick` statements → N+1 segments:
///   `[seg_0, seg_1, ..., seg_N]`
/// where `seg_k` contains the statements between tick k-1 and tick k
/// (and `seg_0` is before the first tick, `seg_N` is after the last tick).
fn split_at_ticks(stmts: &[CHIRStmt]) -> Vec<Vec<CHIRStmt>> {
    let mut segments: Vec<Vec<CHIRStmt>> = vec![Vec::new()];
    for stmt in stmts {
        if matches!(stmt, CHIRStmt::AwaitTick { .. }) {
            segments.push(Vec::new());
        } else {
            segments.last_mut().unwrap().push(stmt.clone());
        }
    }
    segments
}

/// Returns the hardware phase index for a given source segment index.
///
/// Mapping (N = n_ticks):
/// - seg_k for k < N  → phase_k
/// - seg_N (trailing) → phase_{N-1}
fn phase_for_segment(seg_idx: usize, n_ticks: usize) -> usize {
    if seg_idx >= n_ticks {
        n_ticks - 1
    } else {
        seg_idx
    }
}

/// Minimum bit width needed to hold values 0..n_ticks-1.
fn phase_register_width(n_ticks: usize) -> usize {
    if n_ticks <= 1 { 1 }
    else { (usize::BITS - (n_ticks - 1).leading_zeros()) as usize }
}

// ── Helpers: validation ───────────────────────────────────────────────────────

fn validate_seq_chir(seq: &CHIRSeqBody, span: SourceSpan) -> Result<(), SHIRLowerError> {
    // Verify clock consistency in AwaitTick nodes
    for stmt in &seq.loop_body {
        if let CHIRStmt::AwaitTick { clock, span: s } = stmt {
            if clock != &seq.clock {
                return Err(SHIRLowerError::CrossClockTick {
                    expected: seq.clock.clone(),
                    found: clock.clone(),
                    span: *s,
                });
            }
        }
        // Check for ticks inside branches (should be caught by Phase B, but defensive)
        check_no_tick_in_branch(stmt, span)?;
    }
    Ok(())
}

fn check_no_tick_in_branch(stmt: &CHIRStmt, span: SourceSpan) -> Result<(), SHIRLowerError> {
    match stmt {
        CHIRStmt::If { then_body, else_body, span: _s, .. } => {
            for s in then_body {
                if matches!(s, CHIRStmt::AwaitTick { .. }) {
                    return Err(SHIRLowerError::TickInsideBranch { span: *s.span() });
                }
                check_no_tick_in_branch(s, span)?;
            }
            if let Some(eb) = else_body {
                for s in eb {
                    if matches!(s, CHIRStmt::AwaitTick { .. }) {
                        return Err(SHIRLowerError::TickInsideBranch { span: *s.span() });
                    }
                    check_no_tick_in_branch(s, span)?;
                }
            }
            Ok(())
        }
        CHIRStmt::Match { arms, .. } => {
            for arm in arms {
                for s in &arm.body {
                    if matches!(s, CHIRStmt::AwaitTick { .. }) {
                        return Err(SHIRLowerError::TickInsideBranch { span: *s.span() });
                    }
                    check_no_tick_in_branch(s, span)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

// ── Helpers: pre-edge statement lowering ──────────────────────────────────────

/// Lower the wire/port-drive/conditional statements from a segment into `SHIRStmt`s for `pre_edge`.
/// Register assigns and AwaitTick are skipped (they go to post_edge).
/// PortWrite → PortDrive (output port drives are allowed here, including inside branches).
fn lower_pre_edge_stmts(
    stmts: &[CHIRStmt],
    promoted_names: &HashSet<String>,
    renames: &HashMap<String, String>,
) -> Result<Vec<SHIRStmt>, SHIRLowerError> {
    // The sequential pre-edge path has no combinational reassignments (an `Assign`
    // there is a register update handled by the segment logic), so the wire-type
    // map is empty and such assigns are left for that path.
    lower_stmt_list(stmts, promoted_names, renames, &HashMap::new())
}

// ── Helpers: register update extraction ──────────────────────────────────────

/// Extract register updates from a segment's statements.
///
/// Applies `renames` (promoted wire x→x_r) and sequential forwarding so that
/// later assigns within the same segment see the updated values of earlier assigns.
fn extract_reg_updates(
    stmts: &[CHIRStmt],
    registers: &[CHIRRegDecl],
    promoted_names: &HashSet<String>,
    span: SourceSpan,
    renames: &HashMap<String, String>,
) -> Result<Vec<SHIRRegUpdate>, SHIRLowerError> {
    let reg_names: HashSet<String> = registers.iter()
        .map(|r| r.name.clone())
        .chain(promoted_names.iter().map(|n| format!("{}_r", n)))
        .collect();
    let mut forwarding: HashMap<String, SHIRExpr> = HashMap::new();
    extract_updates_from_stmts(stmts, &reg_names, span, renames, &mut forwarding)
}

/// Recursive helper that processes statements in order, threading a forwarding map
/// so sequential assigns within a segment see each other's new values (matching Rust semantics).
///
/// `forwarding`: maps register name → its current effective next-value expression.
/// `renames`: maps promoted wire name → `<name>_r` for the current phase.
/// The value a register ends a branch with: its LAST assignment there, if any.
///
/// A branch's updates are recorded in source order, so a register written more
/// than once appears more than once and only the final write is the branch's
/// result. Reading the first instead drops every later write — silently, since
/// the earlier one is still a well-typed value.
fn last_update_for(updates: &[SHIRRegUpdate], target: &str) -> Option<SHIRExpr> {
    updates
        .iter()
        .rev()
        .find(|u| u.target == target)
        .map(|u| u.next_value.clone())
}

fn extract_updates_from_stmts(
    stmts: &[CHIRStmt],
    reg_names: &HashSet<String>,
    _span: SourceSpan,
    renames: &HashMap<String, String>,
    forwarding: &mut HashMap<String, SHIRExpr>,
) -> Result<Vec<SHIRRegUpdate>, SHIRLowerError> {
    let mut updates: Vec<SHIRRegUpdate> = Vec::new();

    // Helper: resolve an expression — apply renames then substitute forwarded values.
    let resolve = |expr: SHIRExpr, fwd: &HashMap<String, SHIRExpr>| -> SHIRExpr {
        subst_vars(rename_vars(expr, renames), fwd)
    };

    for stmt in stmts {
        match stmt {
            CHIRStmt::Assign { target, value, .. } => {
                let resolved = resolve(lower_expr(value)?, forwarding);
                // Update forwarding so later assigns in this segment see the new value.
                forwarding.insert(target.clone(), resolved.clone());
                updates.push(SHIRRegUpdate { target: target.clone(), next_value: resolved });
            }

            CHIRStmt::If { condition, then_body, else_body, span: s } => {
                let cond_expr = resolve(lower_expr(condition)?, forwarding);

                // Each branch starts from the current forwarding state.
                let mut then_fwd = forwarding.clone();
                let then_updates = extract_updates_from_stmts(then_body, reg_names, *s, renames, &mut then_fwd)?;

                let mut else_fwd = forwarding.clone();
                let else_updates = else_body.as_ref()
                    .map(|eb| extract_updates_from_stmts(eb, reg_names, *s, renames, &mut else_fwd))
                    .transpose()?
                    .unwrap_or_default();

                let mut touched: Vec<String> = Vec::new();
                for u in &then_updates { if !touched.contains(&u.target) { touched.push(u.target.clone()); } }
                for u in &else_updates { if !touched.contains(&u.target) { touched.push(u.target.clone()); } }

                for target in touched {
                    // Missing branch → hold: use current forwarding value or Var(target).
                    let hold = || forwarding.get(&target).cloned()
                        .unwrap_or_else(|| SHIRExpr::Var(target.clone()));

                    // The LAST assignment in the branch is the branch's result.
                    // Taking the first silently discarded a re-assignment, which is
                    // exactly the mod-N counter idiom `t = t + 1; if t == N { t = 0 }`
                    // — the reset vanished and the counter ran free.
                    let then_val = last_update_for(&then_updates, &target)
                        .unwrap_or_else(hold);

                    let else_val = last_update_for(&else_updates, &target)
                        .unwrap_or_else(hold);

                    let mux_val = SHIRExpr::Mux {
                        cond: Box::new(cond_expr.clone()),
                        then_val: Box::new(then_val),
                        else_val: Box::new(else_val),
                    };
                    forwarding.insert(target.clone(), mux_val.clone());
                    updates.push(SHIRRegUpdate { target, next_value: mux_val });
                }
            }

            CHIRStmt::Match { scrutinee, arms, span: s } => {
                let scrutinee_expr = resolve(lower_expr(scrutinee)?, forwarding);

                // Process each arm independently from the current forwarding state.
                let arm_updates: Vec<Vec<SHIRRegUpdate>> = arms.iter()
                    .map(|arm| {
                        let mut arm_fwd = forwarding.clone();
                        extract_updates_from_stmts(&arm.body, reg_names, *s, renames, &mut arm_fwd)
                    })
                    .collect::<Result<_, _>>()?;

                let mut touched: Vec<String> = Vec::new();
                for arm_upds in &arm_updates {
                    for u in arm_upds {
                        if !touched.contains(&u.target) { touched.push(u.target.clone()); }
                    }
                }

                for target in touched {
                    let hold = || forwarding.get(&target).cloned()
                        .unwrap_or_else(|| SHIRExpr::Var(target.clone()));

                    let mut case_arms: Vec<SHIRCaseArm> = Vec::new();
                    let mut default_val: Option<SHIRExpr> = None;

                    for (arm, upds) in arms.iter().zip(arm_updates.iter()) {
                        // Last assignment wins — see the note in the `If` arm.
                        let next_val = last_update_for(upds, &target).unwrap_or_else(hold);

                        let guard = arm.guard.as_ref()
                            .map(|g| Ok(resolve(lower_expr(g)?, forwarding)))
                            .transpose()?;

                        let is_default = arm.patterns.iter().any(|p| matches!(p, CHIRPattern::Wildcard))
                            && guard.is_none();

                        if is_default {
                            default_val = Some(next_val);
                        } else {
                            for pattern in &arm.patterns {
                                case_arms.push(SHIRCaseArm {
                                    pattern: lower_pattern(pattern)?,
                                    guard: guard.clone(),
                                    value: next_val.clone(),
                                });
                            }
                        }
                    }

                    let default = default_val.unwrap_or_else(hold);
                    let case_val = SHIRExpr::Case {
                        scrutinee: Box::new(scrutinee_expr.clone()),
                        arms: case_arms,
                        default: Box::new(default),
                    };
                    forwarding.insert(target.clone(), case_val.clone());
                    updates.push(SHIRRegUpdate { target, next_value: case_val });
                }
            }

            // Wire, Emit, AwaitTick — not register updates
            _ => {}
        }
    }

    Ok(updates)
}

// ── Helpers: wire promotion ───────────────────────────────────────────────────

/// Find wires that need to be promoted to registers.
///
/// A wire declared in seg_j needs promotion if it is referenced in a segment
/// that maps to a DIFFERENT hardware phase than seg_j.
///
/// Returns a list of (wire_name, wire_ty, wire_init) for promoted wires.
/// `wire_init` is always `None` — promoted wires start at 0 (unknown initial).
fn find_promoted_wires(
    segments: &[Vec<CHIRStmt>],
    wire_segment: &HashMap<String, usize>,
    n_ticks: usize,
) -> Vec<(String, CHIRType, Option<SHIRLit>)> {
    // Collect all wire types from declarations
    let wire_types: HashMap<String, CHIRType> = segments.iter()
        .flat_map(|stmts| stmts.iter())
        .filter_map(|s| {
            if let CHIRStmt::Wire { name, ty, .. } = s {
                Some((name.clone(), ty.clone()))
            } else {
                None
            }
        })
        .collect();

    // Find which wires are referenced in a different phase from their declaration
    let mut promote: HashSet<String> = HashSet::new();

    for (seg_idx, stmts) in segments.iter().enumerate() {
        let seg_phase = phase_for_segment(seg_idx, n_ticks);
        for stmt in stmts {
            collect_expr_vars_in_stmt(stmt, &mut |var_name: &str| {
                if let Some(&decl_seg) = wire_segment.get(var_name) {
                    let decl_phase = phase_for_segment(decl_seg, n_ticks);
                    if decl_phase != seg_phase {
                        promote.insert(var_name.to_string());
                    }
                }
            });
        }
    }

    promote.into_iter()
        .filter_map(|name| {
            let ty = wire_types.get(&name)?.clone();
            Some((name, ty, None))
        })
        .collect()
}

/// Walk a statement and call `visitor` on every variable name in every expression.
fn collect_expr_vars_in_stmt(stmt: &CHIRStmt, visitor: &mut impl FnMut(&str)) {
    match stmt {
        CHIRStmt::Wire { value, .. } => collect_expr_vars(value, visitor),
        CHIRStmt::Assign { value, .. } => collect_expr_vars(value, visitor),
        CHIRStmt::PortWrite { value, .. } => collect_expr_vars(value, visitor),
        CHIRStmt::If { condition, then_body, else_body, .. } => {
            collect_expr_vars(condition, visitor);
            for s in then_body { collect_expr_vars_in_stmt(s, visitor); }
            if let Some(eb) = else_body { for s in eb { collect_expr_vars_in_stmt(s, visitor); } }
        }
        CHIRStmt::Match { scrutinee, arms, .. } => {
            collect_expr_vars(scrutinee, visitor);
            for arm in arms {
                if let Some(g) = &arm.guard { collect_expr_vars(g, visitor); }
                for s in &arm.body { collect_expr_vars_in_stmt(s, visitor); }
            }
        }
        CHIRStmt::ForLoop { start, end, body, .. } => {
            collect_expr_vars(start, visitor);
            collect_expr_vars(end, visitor);
            for s in body { collect_expr_vars_in_stmt(s, visitor); }
        }
        CHIRStmt::IndexAssign { base, index, value, .. } => {
            visitor(base);
            collect_expr_vars(index, visitor);
            collect_expr_vars(value, visitor);
        }
        CHIRStmt::MemRead { addr, .. } => collect_expr_vars(addr, visitor),
        CHIRStmt::MemWrite { addr, value, .. } => {
            collect_expr_vars(addr, visitor);
            collect_expr_vars(value, visitor);
        }
        CHIRStmt::AwaitTick { .. } => {}
    }
}

fn collect_expr_vars(expr: &CHIRExpr, visitor: &mut impl FnMut(&str)) {
    match expr {
        CHIRExpr::Var(name) => visitor(name),
        CHIRExpr::Lit(_) => {}
        CHIRExpr::BinOp { left, right, .. } => {
            collect_expr_vars(left, visitor);
            collect_expr_vars(right, visitor);
        }
        CHIRExpr::UnOp { expr, .. } => collect_expr_vars(expr, visitor),
        CHIRExpr::Mux { cond, then_val, else_val } => {
            collect_expr_vars(cond, visitor);
            collect_expr_vars(then_val, visitor);
            collect_expr_vars(else_val, visitor);
        }
        CHIRExpr::Case { scrutinee, arms, default } => {
            collect_expr_vars(scrutinee, visitor);
            for arm in arms {
                if let Some(g) = &arm.guard { collect_expr_vars(g, visitor); }
                collect_expr_vars(&arm.value, visitor);
            }
            if let Some(d) = default { collect_expr_vars(d, visitor); }
        }
        CHIRExpr::Concat(exprs) => { for e in exprs { collect_expr_vars(e, visitor); } }
        CHIRExpr::Slice { expr, .. } => collect_expr_vars(expr, visitor),
        CHIRExpr::DynBit { base, index } => {
            collect_expr_vars(base, visitor);
            collect_expr_vars(index, visitor);
        }
        CHIRExpr::Resize { expr, .. } => collect_expr_vars(expr, visitor),
        // A memory read result is not a variable reference.
        CHIRExpr::MemData { .. } | CHIRExpr::MemValid { .. } => {}
    }
}

// ── Expression lowering ───────────────────────────────────────────────────────

/// Substitute variable references with expressions (used for sequential forwarding).
fn subst_vars(expr: SHIRExpr, subst: &HashMap<String, SHIRExpr>) -> SHIRExpr {
    if subst.is_empty() { return expr; }
    match expr {
        SHIRExpr::Var(ref name) => subst.get(name).cloned().unwrap_or(expr),
        SHIRExpr::Lit(_)
        | SHIRExpr::PhaseEq(_)
        | SHIRExpr::MemData { .. }
        | SHIRExpr::MemValid { .. } => expr,
        SHIRExpr::BinOp { left, op, right } => SHIRExpr::BinOp {
            left: Box::new(subst_vars(*left, subst)),
            op,
            right: Box::new(subst_vars(*right, subst)),
        },
        SHIRExpr::UnOp { op, expr } => SHIRExpr::UnOp {
            op,
            expr: Box::new(subst_vars(*expr, subst)),
        },
        SHIRExpr::Mux { cond, then_val, else_val } => SHIRExpr::Mux {
            cond: Box::new(subst_vars(*cond, subst)),
            then_val: Box::new(subst_vars(*then_val, subst)),
            else_val: Box::new(subst_vars(*else_val, subst)),
        },
        SHIRExpr::Case { scrutinee, arms, default } => SHIRExpr::Case {
            scrutinee: Box::new(subst_vars(*scrutinee, subst)),
            arms: arms.into_iter().map(|a| SHIRCaseArm {
                pattern: a.pattern,
                guard: a.guard.map(|g| subst_vars(g, subst)),
                value: subst_vars(a.value, subst),
            }).collect(),
            default: Box::new(subst_vars(*default, subst)),
        },
        SHIRExpr::Concat(exprs) => SHIRExpr::Concat(
            exprs.into_iter().map(|e| subst_vars(e, subst)).collect()
        ),
        SHIRExpr::Slice { expr, high, low } => SHIRExpr::Slice {
            expr: Box::new(subst_vars(*expr, subst)),
            high,
            low,
        },
        SHIRExpr::DynBit { base, index } => SHIRExpr::DynBit {
            base: Box::new(subst_vars(*base, subst)),
            index: Box::new(subst_vars(*index, subst)),
        },
        SHIRExpr::Resize { expr, width } => SHIRExpr::Resize {
            expr: Box::new(subst_vars(*expr, subst)),
            width,
        },
    }
}

/// Rename variables using a name→name map (used for promoted wire substitution).
fn rename_vars(expr: SHIRExpr, renames: &HashMap<String, String>) -> SHIRExpr {
    if renames.is_empty() { return expr; }
    let subst: HashMap<String, SHIRExpr> = renames.iter()
        .map(|(k, v)| (k.clone(), SHIRExpr::Var(v.clone())))
        .collect();
    subst_vars(expr, &subst)
}

/// Convert a `CHIRExpr` to a `SHIRExpr`.
///
/// This is mostly structural. The key differences:
/// - `CHIRExpr::Case::default` is `Option`; `SHIRExpr::Case::default` is required (`Box`).
///   If the CHIR case has no default, we error — Phase B's scope validation should have caught
///   non-exhaustive matches, but we may hit this for expressions.
pub fn lower_expr(expr: &CHIRExpr) -> Result<SHIRExpr, SHIRLowerError> {
    match expr {
        CHIRExpr::Var(name) => Ok(SHIRExpr::Var(name.clone())),
        CHIRExpr::Lit(lit) => Ok(SHIRExpr::Lit(lower_lit(lit))),
        CHIRExpr::BinOp { left, op, right } => Ok(SHIRExpr::BinOp {
            left: Box::new(lower_expr(left)?),
            op: op.clone(),
            right: Box::new(lower_expr(right)?),
        }),
        CHIRExpr::UnOp { op, expr } => Ok(SHIRExpr::UnOp {
            op: op.clone(),
            expr: Box::new(lower_expr(expr)?),
        }),
        CHIRExpr::Mux { cond, then_val, else_val } => Ok(SHIRExpr::Mux {
            cond: Box::new(lower_expr(cond)?),
            then_val: Box::new(lower_expr(then_val)?),
            else_val: Box::new(lower_expr(else_val)?),
        }),
        CHIRExpr::Case { scrutinee, arms, default } => {
            let default_expr = match default {
                Some(d) => lower_expr(d)?,
                None => return Err(SHIRLowerError::UnsupportedConstruct {
                    description: "match-as-expression without a wildcard/default arm".to_string(),
                    span: SourceSpan::default(),
                }),
            };
            Ok(SHIRExpr::Case {
                scrutinee: Box::new(lower_expr(scrutinee)?),
                arms: arms.iter()
                    .map(|a| Ok(SHIRCaseArm {
                        pattern: lower_pattern(&a.pattern)?,
                        guard: a.guard.as_ref().map(lower_expr).transpose()?,
                        value: lower_expr(&a.value)?,
                    }))
                    .collect::<Result<_, _>>()?,
                default: Box::new(default_expr),
            })
        }
        CHIRExpr::Concat(exprs) => Ok(SHIRExpr::Concat(
            exprs.iter().map(lower_expr).collect::<Result<_, _>>()?
        )),
        CHIRExpr::Slice { expr, high, low } => Ok(SHIRExpr::Slice {
            expr: Box::new(lower_expr(expr)?),
            high: *high,
            low: *low,
        }),
        CHIRExpr::DynBit { base, index } => Ok(SHIRExpr::DynBit {
            base: Box::new(lower_expr(base)?),
            index: Box::new(lower_expr(index)?),
        }),
        CHIRExpr::Resize { expr, width } => Ok(SHIRExpr::Resize {
            expr: Box::new(lower_expr(expr)?),
            width: width.clone(),
        }),
        CHIRExpr::MemData { mem, port } => {
            Ok(SHIRExpr::MemData { mem: mem.clone(), port: *port })
        }
        CHIRExpr::MemValid { mem, port } => {
            Ok(SHIRExpr::MemValid { mem: mem.clone(), port: *port })
        }
    }
}

fn lower_lit(lit: &CHIRLit) -> SHIRLit {
    SHIRLit { ty: lit.ty.clone(), value: lit.value }
}

fn lower_pattern(pattern: &CHIRPattern) -> Result<SHIRPattern, SHIRLowerError> {
    match pattern {
        CHIRPattern::Lit(lit) => Ok(SHIRPattern::Lit(lower_lit(lit))),
        CHIRPattern::Wildcard => Ok(SHIRPattern::Wildcard),
        CHIRPattern::Tuple(parts) => {
            let lowered = parts.iter().map(lower_pattern).collect::<Result<_, _>>()?;
            Ok(SHIRPattern::Tuple(lowered))
        }
        CHIRPattern::EnumVariant { name, inner } => Ok(SHIRPattern::EnumVariant {
            name: name.clone(),
            inner: inner.as_ref()
                .map(|p| lower_pattern(p).map(Box::new))
                .transpose()?,
        }),
    }
}

/// A memory preload, expression by expression.
fn lower_mem_init(init: &CHIRMemInit) -> Result<SHIRMemInit, SHIRLowerError> {
    Ok(match init {
        CHIRMemInit::Fill { var, value } => SHIRMemInit::Fill {
            var: var.clone(),
            value: lower_expr(value)?,
        },
        CHIRMemInit::Words(words) => {
            SHIRMemInit::Words(words.iter().map(lower_expr).collect::<Result<_, _>>()?)
        }
    })
}

fn lower_reg_decl(r: &CHIRRegDecl) -> Result<SHIRReg, SHIRLowerError> {
    Ok(SHIRReg {
        name: r.name.clone(),
        ty: r.ty.clone(),
        init: r.init.as_ref().map(|lit| SHIRLit { ty: lit.ty.clone(), value: lit.value }),
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::chir::{
        CHIRBody, CHIRLit, CHIRModule, CHIRPortDir, CHIRPortKind, CHIRSeqBody,
        CHIRType, Width,
    };
    use copper_core::frontend_ir::SourceSpan;

    fn span() -> SourceSpan { SourceSpan::default() }

    fn no_hw() -> std::collections::HashSet<String> { Default::default() }

    fn make_chir(src: &str) -> CHIRModule {
        use crate::parser::capture_frontend_ir;
        use crate::chir_lower::{lower_to_chir, ModuleRegistry};
        let design_fn: syn::ItemFn = syn::parse_str(src).unwrap();
        let fir = capture_frontend_ir(&design_fn, &no_hw()).unwrap();
        let registry = ModuleRegistry::new();
        lower_to_chir(&fir, &no_hw(), &registry).unwrap()
    }

    // ── Phase register width ─────────────────────────────────────────────────

    #[test]
    fn test_phase_register_width_two_phases() {
        assert_eq!(phase_register_width(2), 1);
    }

    #[test]
    fn test_phase_register_width_four_phases() {
        assert_eq!(phase_register_width(4), 2);
    }

    #[test]
    fn test_phase_register_width_three_phases() {
        assert_eq!(phase_register_width(3), 2);
    }

    // ── Segment splitting ────────────────────────────────────────────────────

    #[test]
    fn test_split_single_tick_gives_two_segments() {
        let stmts = vec![
            CHIRStmt::Assign {
                target: "x".to_string(),
                value: CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(8) }, value: 0 }),
                span: span(),
            },
            CHIRStmt::AwaitTick { clock: "clk".to_string(), span: span() },
            CHIRStmt::Assign {
                target: "x".to_string(),
                value: CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(8) }, value: 1 }),
                span: span(),
            },
        ];
        let segments = split_at_ticks(&stmts);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].len(), 1);
        assert_eq!(segments[1].len(), 1);
    }

    #[test]
    fn test_split_two_ticks_gives_three_segments() {
        let stmts = vec![
            CHIRStmt::AwaitTick { clock: "clk".to_string(), span: span() },
            CHIRStmt::AwaitTick { clock: "clk".to_string(), span: span() },
        ];
        let segments = split_at_ticks(&stmts);
        assert_eq!(segments.len(), 3);
    }

    // ── Phase mapping ────────────────────────────────────────────────────────

    #[test]
    fn test_phase_for_segment_single_tick() {
        // n_ticks=1: seg_0→phase_0, seg_1 (trailing)→phase_0
        assert_eq!(phase_for_segment(0, 1), 0);
        assert_eq!(phase_for_segment(1, 1), 0);
    }

    #[test]
    fn test_phase_for_segment_two_ticks() {
        // n_ticks=2: seg_0→phase_0, seg_1→phase_1, seg_2 (trailing)→phase_1
        assert_eq!(phase_for_segment(0, 2), 0);
        assert_eq!(phase_for_segment(1, 2), 1);
        assert_eq!(phase_for_segment(2, 2), 1);
    }

    // ── Combinational body ───────────────────────────────────────────────────

    #[test]
    fn test_lower_comb_simple_adder() {
        let chir = make_chir("fn add(a: u8, b: u8) -> u8 { a + b }");
        let shir = lower_to_shir(&chir).unwrap();
        assert_eq!(shir.name, "add");
        assert!(matches!(shir.body, SHIRBody::Combinational(_)));
        if let SHIRBody::Combinational(body) = &shir.body {
            // Final expression `a + b` becomes PortDrive to "out"
            assert_eq!(body.stmts.len(), 1);
            if let SHIRStmt::PortDrive { port_name, value } = &body.stmts[0] {
                assert_eq!(port_name, "out");
                assert!(matches!(value, SHIRExpr::BinOp { .. }));
            } else {
                panic!("expected PortDrive");
            }
        }
    }

    #[test]
    fn test_lower_comb_single_var_output() {
        let chir = make_chir("fn pass(a: u8) -> u8 { a }");
        let shir = lower_to_shir(&chir).unwrap();
        if let SHIRBody::Combinational(body) = &shir.body {
            assert_eq!(body.stmts.len(), 1);
            if let SHIRStmt::PortDrive { value, .. } = &body.stmts[0] {
                assert!(matches!(value, SHIRExpr::Var(_)));
            } else {
                panic!("expected PortDrive");
            }
        } else {
            panic!("expected combinational");
        }
    }

    // ── Sequential body ──────────────────────────────────────────────────────

    #[test]
    fn test_lower_seq_single_tick_register_update() {
        let chir = make_chir(
            "async fn counter(clk: Clock<MainClk>) {
                let mut count: u8 = 0u8;
                loop {
                    count = count.wrapping_add(1u8);
                    clk.tick().await;
                }
            }"
        );
        let shir = lower_to_shir(&chir).unwrap();
        if let SHIRBody::Sequential(body) = &shir.body {
            assert_eq!(body.phases.len(), 1);
            assert_eq!(body.clock, "clk");
            assert_eq!(body.registers.len(), 1);
            assert_eq!(body.registers[0].name, "count");
            // post_edge should have the count update
            let phase = &body.phases[0];
            assert!(phase.post_edge.iter().any(|u| u.target == "count"));
        } else {
            panic!("expected sequential");
        }
    }

    #[test]
    fn test_lower_seq_single_phase_has_no_phase_r() {
        let chir = make_chir(
            "async fn m(clk: Clock<MainClk>, x: u8) {
                loop { clk.tick().await; }
            }"
        );
        let shir = lower_to_shir(&chir).unwrap();
        if let SHIRBody::Sequential(body) = &shir.body {
            assert_eq!(body.phases.len(), 1);
            assert!(!body.registers.iter().any(|r| r.name == "phase_r"));
        }
    }

    #[test]
    fn test_lower_seq_two_ticks_generates_phase_r() {
        let chir = make_chir(
            "async fn m(clk: Clock<MainClk>, x: u8) {
                let mut acc: u8 = 0u8;
                loop {
                    clk.tick().await;
                    clk.tick().await;
                    acc = acc.wrapping_add(x);
                }
            }"
        );
        let shir = lower_to_shir(&chir).unwrap();
        if let SHIRBody::Sequential(body) = &shir.body {
            assert_eq!(body.phases.len(), 2);
            assert!(body.registers.iter().any(|r| r.name == "phase_r"));
            // phase_r should start at 0
            let phase_r = body.registers.iter().find(|r| r.name == "phase_r").unwrap();
            assert_eq!(phase_r.init.as_ref().unwrap().value, 0);
        } else {
            panic!("expected sequential");
        }
    }

    #[test]
    fn test_lower_seq_phase_advance_in_post_edge() {
        let chir = make_chir(
            "async fn m(clk: Clock<MainClk>, x: u8) {
                let mut acc: u8 = 0u8;
                loop {
                    clk.tick().await;
                    clk.tick().await;
                    acc = acc.wrapping_add(x);
                }
            }"
        );
        let shir = lower_to_shir(&chir).unwrap();
        if let SHIRBody::Sequential(body) = &shir.body {
            // Phase 0 post_edge should advance to phase 1
            let phase0 = &body.phases[0];
            let advance = phase0.post_edge.iter().find(|u| u.target == "phase_r");
            assert!(advance.is_some());
            match &advance.unwrap().next_value {
                SHIRExpr::Lit(lit) => assert_eq!(lit.value, 1),
                _ => panic!("expected literal 1 for phase advance"),
            }
            // Phase 1 post_edge should wrap back to phase 0
            let phase1 = &body.phases[1];
            let wrap = phase1.post_edge.iter().find(|u| u.target == "phase_r");
            assert!(wrap.is_some());
            match &wrap.unwrap().next_value {
                SHIRExpr::Lit(lit) => assert_eq!(lit.value, 0),
                _ => panic!("expected literal 0 for phase wrap"),
            }
        }
    }

    // ── Conditional register updates ─────────────────────────────────────────

    #[test]
    fn test_one_sided_if_holds_current_value() {
        // if cond { count = 0u8; }  → Mux(cond, 0, count)
        let stmts = vec![CHIRStmt::If {
            condition: CHIRExpr::Var("cond".to_string()),
            then_body: vec![CHIRStmt::Assign {
                target: "count".to_string(),
                value: CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(8) }, value: 0 }),
                span: span(),
            }],
            else_body: None,
            span: span(),
        }];

        let regs = vec![CHIRRegDecl {
            name: "count".to_string(),
            ty: CHIRType::UInt { width: Width::Concrete(8) },
            init: None,
            span: span(),
        }];
        let promoted = std::collections::HashSet::new();

        let updates = extract_reg_updates(&stmts, &regs, &promoted, span(), &HashMap::new()).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].target, "count");
        // next_value should be Mux(cond, 0, count)
        assert!(matches!(&updates[0].next_value, SHIRExpr::Mux { .. }));
        if let SHIRExpr::Mux { else_val, .. } = &updates[0].next_value {
            // else_val should be Var("count") — hold
            assert!(matches!(else_val.as_ref(), SHIRExpr::Var(n) if n == "count"));
        }
    }

    #[test]
    fn test_two_sided_if_uses_both_branches() {
        let stmts = vec![CHIRStmt::If {
            condition: CHIRExpr::Var("cond".to_string()),
            then_body: vec![CHIRStmt::Assign {
                target: "x".to_string(),
                value: CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(8) }, value: 1 }),
                span: span(),
            }],
            else_body: Some(vec![CHIRStmt::Assign {
                target: "x".to_string(),
                value: CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(8) }, value: 2 }),
                span: span(),
            }]),
            span: span(),
        }];

        let regs = vec![CHIRRegDecl {
            name: "x".to_string(),
            ty: CHIRType::UInt { width: Width::Concrete(8) },
            init: None,
            span: span(),
        }];
        let promoted = std::collections::HashSet::new();

        let updates = extract_reg_updates(&stmts, &regs, &promoted, span(), &HashMap::new()).unwrap();
        assert_eq!(updates.len(), 1);
        assert!(matches!(&updates[0].next_value, SHIRExpr::Mux { .. }));
        if let SHIRExpr::Mux { then_val, else_val, .. } = &updates[0].next_value {
            assert!(matches!(then_val.as_ref(), SHIRExpr::Lit(l) if l.value == 1));
            assert!(matches!(else_val.as_ref(), SHIRExpr::Lit(l) if l.value == 2));
        }
    }

    // ── End-to-end ───────────────────────────────────────────────────────────

    #[test]
    fn test_e2e_counter_single_tick() {
        let chir = make_chir(
            "async fn counter(clk: Clock<MainClk>) {
                let mut count: u8 = 0u8;
                loop {
                    count = count.wrapping_add(1u8);
                    clk.tick().await;
                }
            }"
        );
        let shir = lower_to_shir(&chir).unwrap();
        assert_eq!(shir.name, "counter");
        if let SHIRBody::Sequential(body) = &shir.body {
            assert_eq!(body.phases.len(), 1);
            assert!(!body.registers.iter().any(|r| r.name == "phase_r"));
            let phase = &body.phases[0];
            assert!(phase.post_edge.iter().any(|u| u.target == "count"));
        } else {
            panic!("expected sequential");
        }
    }

    #[test]
    fn test_e2e_comb_adder_ports_preserved() {
        let chir = make_chir("fn add(a: u8, b: u8) -> u8 { a + b }");
        let shir = lower_to_shir(&chir).unwrap();
        let port_names: Vec<_> = shir.ports.iter().map(|p| p.name.as_str()).collect();
        assert!(port_names.contains(&"a"));
        assert!(port_names.contains(&"b"));
        assert!(port_names.contains(&"out"));
    }

    // ── Sequential forwarding ────────────────────────────────────────────────

    #[test]
    fn test_sequential_forwarding_direct_assigns() {
        // stage1_r = input + 1;
        // stage2_r = stage1_r + stage1_r;
        // stage2_r's next_value should use the new stage1_r (input+1), not the old register.
        let stmts = vec![
            CHIRStmt::Assign {
                target: "stage1_r".to_string(),
                value: CHIRExpr::BinOp {
                    left: Box::new(CHIRExpr::Var("input".to_string())),
                    op: copper_core::chir::CHIRBinOp::Add { wrapping: true },
                    right: Box::new(CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(8) }, value: 1 })),
                },
                span: span(),
            },
            CHIRStmt::Assign {
                target: "stage2_r".to_string(),
                value: CHIRExpr::BinOp {
                    left: Box::new(CHIRExpr::Var("stage1_r".to_string())),
                    op: copper_core::chir::CHIRBinOp::Add { wrapping: true },
                    right: Box::new(CHIRExpr::Var("stage1_r".to_string())),
                },
                span: span(),
            },
        ];
        let regs = vec![
            CHIRRegDecl { name: "stage1_r".to_string(), ty: CHIRType::UInt { width: Width::Concrete(8) }, init: None, span: span() },
            CHIRRegDecl { name: "stage2_r".to_string(), ty: CHIRType::UInt { width: Width::Concrete(8) }, init: None, span: span() },
        ];
        let promoted = std::collections::HashSet::new();
        let updates = extract_reg_updates(&stmts, &regs, &promoted, span(), &HashMap::new()).unwrap();

        assert_eq!(updates.len(), 2);
        let s1 = updates.iter().find(|u| u.target == "stage1_r").unwrap();
        let s2 = updates.iter().find(|u| u.target == "stage2_r").unwrap();

        // stage1_r's next_value = input + 1 (unchanged)
        assert!(matches!(&s1.next_value, SHIRExpr::BinOp { .. }));

        // stage2_r's next_value must NOT be BinOp(Var("stage1_r"), ...) — that would use the OLD register.
        // It should be BinOp(BinOp(input+1), ...) — forwarded to the new stage1_r value.
        match &s2.next_value {
            SHIRExpr::BinOp { left, right, .. } => {
                // Both sides should be the forwarded expression (input+1), not Var("stage1_r")
                assert!(!matches!(left.as_ref(), SHIRExpr::Var(n) if n == "stage1_r"),
                    "stage2_r left operand should be forwarded, not bare Var(stage1_r)");
                assert!(!matches!(right.as_ref(), SHIRExpr::Var(n) if n == "stage1_r"),
                    "stage2_r right operand should be forwarded, not bare Var(stage1_r)");
            }
            other => panic!("expected BinOp for stage2_r next_value, got {:?}", other),
        }
    }

    #[test]
    fn test_promoted_wire_renamed_in_later_phase() {
        // Two-tick module: step1 computed in phase_0, used in phase_1.
        // phase_1's expressions should reference step1_r, not step1.
        let chir = make_chir(
            "async fn two_cycle_op(clk: Clock<MainClk>, input: u8) {
                let mut acc: u8 = 0u8;
                loop {
                    clk.tick().await;
                    let step1: u8 = input.wrapping_add(1u8);
                    clk.tick().await;
                    acc = step1.wrapping_add(step1);
                }
            }"
        );
        let shir = lower_to_shir(&chir).unwrap();
        if let SHIRBody::Sequential(body) = &shir.body {
            assert_eq!(body.phases.len(), 2);
            // step1 declared in seg_1 (phase_1), used in seg_2 (trailing, also phase_1)
            // Same phase → no promotion needed; acc should use Var("step1") directly.
            let phase1 = &body.phases[1];
            let acc_update = phase1.post_edge.iter().find(|u| u.target == "acc").unwrap();
            // next_value should reference step1 (wire), not step1_r (promoted register)
            fn contains_step1_r(expr: &SHIRExpr) -> bool {
                match expr {
                    SHIRExpr::Var(n) => n == "step1_r",
                    SHIRExpr::BinOp { left, right, .. } => contains_step1_r(left) || contains_step1_r(right),
                    _ => false,
                }
            }
            assert!(!contains_step1_r(&acc_update.next_value),
                "acc should use wire step1, not promoted step1_r (same phase — no register needed)");
        } else {
            panic!("expected sequential");
        }
    }

    #[test]
    fn test_forwarding_with_one_sided_if() {
        // R = 5; if cond { R = R + 1; } → after if, R's next = Mux(cond, 5+1, 5)
        let stmts = vec![
            CHIRStmt::Assign {
                target: "R".to_string(),
                value: CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(8) }, value: 5 }),
                span: span(),
            },
            CHIRStmt::If {
                condition: CHIRExpr::Var("cond".to_string()),
                then_body: vec![CHIRStmt::Assign {
                    target: "R".to_string(),
                    value: CHIRExpr::BinOp {
                        left: Box::new(CHIRExpr::Var("R".to_string())),
                        op: copper_core::chir::CHIRBinOp::Add { wrapping: false },
                        right: Box::new(CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(8) }, value: 1 })),
                    },
                    span: span(),
                }],
                else_body: None,
                span: span(),
            },
        ];
        let regs = vec![CHIRRegDecl { name: "R".to_string(), ty: CHIRType::UInt { width: Width::Concrete(8) }, init: None, span: span() }];
        let promoted = std::collections::HashSet::new();
        let updates = extract_reg_updates(&stmts, &regs, &promoted, span(), &HashMap::new()).unwrap();

        // Should produce two updates for R: first the flat assign, then the Mux
        // (or deduplicated to just the Mux — either is acceptable as long as last wins)
        let last_r = updates.iter().rev().find(|u| u.target == "R").unwrap();
        // The final R update must be a Mux (the if overrides the flat assign)
        assert!(matches!(&last_r.next_value, SHIRExpr::Mux { .. }),
            "expected Mux for R after one-sided if with prior assign");
        if let SHIRExpr::Mux { then_val, else_val, .. } = &last_r.next_value {
            // then_val: R+1 where R was already forwarded to 5 → should be Lit(5)+1, not Var("R")+1
            assert!(!matches!(then_val.as_ref(), SHIRExpr::BinOp { left, .. } if matches!(left.as_ref(), SHIRExpr::Var(n) if n == "R")),
                "then_val should use forwarded R=5, not bare Var(R)");
            // else_val: hold = forwarded R = Lit(5)
            assert!(matches!(else_val.as_ref(), SHIRExpr::Lit(l) if l.value == 5),
                "else_val (hold) should be the forwarded value 5");
        }
    }
}
