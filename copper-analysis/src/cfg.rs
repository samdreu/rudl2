//! The intra-module **control-flow graph** (CFG) — item 2 of
//! `design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`, the analysis both the sim
//! macro and the transpiler consume (c2, gate G6).
//!
//! A `#[hardware(sequential)]` module is an `async fn` whose body is a top-level
//! `loop`. This module models that loop as a real CFG over the `syn` AST and runs
//! two analyses on it:
//!
//! * **Backward liveness → register inference** ([`Cfg::registers`]). A local is a
//!   flip-flop iff (a) it is *defined inside the loop* and (b) its value is *live
//!   across a clock edge* — the pre-tick value is read post-tick. This is the T1
//!   answer (the synthesizable register set computed from control flow, not read
//!   off rustc's over-capturing `Future` layout), and it generalizes the G6 slice's
//!   minimal "pre-loop binding reassigned in loop" criterion to registers *born
//!   inside* the loop and live across an interior `.await` (e.g. `mac_pipeline`'s
//!   pipeline registers).
//!
//! * **Reachability well-formedness** ([`Cfg::check_reachability`]). Copper's core
//!   invariant — *every path through a hardware loop must eventually reach a tick*
//!   — is not checked anywhere today; it holds only as an accident of the
//!   single-trailing-tick construction. Here it is a real analysis: delete every
//!   tick edge and the reachable subgraph must be acyclic (a DFS back-edge check).
//!   A tickless cycle is a zero-time combinational loop and is rejected.
//!
//! **Edges** are classified [`EdgeKind::Comb`] (same-cycle control flow) or
//! [`EdgeKind::Tick`] (the edge *out of* a `clk.tick().await`, a clock-cycle
//! boundary). Tick edges are labeled with the clock **receiver identity** (the
//! groundwork item 4 needs to tag which domain a boundary belongs to — today's
//! `control_extract.rs` matches ticks by method name only, losing the receiver).
//!
//! **v1 scope.** Straight-line code, statement-position `if`/`else` (incl.
//! `else if`) and `match` arms, and ticks anywhere among them. A genuine
//! basic-block builder for *nested* loops (AST duplication doesn't terminate on
//! back-edges) is the one follow-on phase after v1 lands and is verified — a nested
//! loop is folded here into a single opaque node.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Span, TokenStream, TokenTree};
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{BinOp, Expr, ItemFn, Pat, Stmt};

/// How control reaches the successor of a CFG node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EdgeKind {
    /// Same clock cycle — ordinary (combinational) control flow.
    Comb,
    /// Crosses a clock edge — the edge *out of* a `clk.tick().await`.
    Tick,
}

/// One **memory-port access site**, as it is spelled in the source.
///
/// The transpiler's memory rules are all about *when* an access happens relative
/// to a clock edge, so they have to be decided where the edges are still visible:
/// on the source. Downstream of `control_extract` they are not — a body whose
/// ticks live inside branches or loops is rewritten into a single-tick `match pc`
/// FSM, which is exactly the shape a segment-splitting check reads as "everything
/// happens in one cycle". See [`Cfg::check_memory_staging`].
#[derive(Clone, PartialEq, Eq, Debug)]
struct MemAccess {
    /// The memory local's name (`mem` in `mem.read_port::<0>()`).
    mem: String,
    /// The port index as written in the turbofish, kept as text: an index that is
    /// not a literal belongs to a generic memory, which the transpiler refuses
    /// downstream anyway, and text keeps the two spellings distinct meanwhile.
    port: String,
    kind: MemAccessKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MemAccessKind {
    /// `read_port::<K>().read(addr)` — drives the port's address bus this cycle.
    StageRead,
    /// `write_port::<K>().write(addr, v)` — drives the write bus this cycle.
    StageWrite,
    /// `read_port::<K>().data()` / `.is_ready()` — observes what the *edge*
    /// produced, so it belongs to a later cycle than the staging that fed it.
    Observe(&'static str),
}

impl MemAccessKind {
    /// The physical bus an access drives, or `None` for an observation (which
    /// drives nothing and so never conflicts).
    fn bus(self) -> Option<bool> {
        match self {
            MemAccessKind::StageRead => Some(true),
            MemAccessKind::StageWrite => Some(false),
            MemAccessKind::Observe(_) => None,
        }
    }
}

/// One CFG node. Kept at single-statement granularity so `defs`/`uses` are clean
/// for the liveness dataflow (use-before-def within a node is handled by the
/// `use ∪ (out − def)` transfer, so `let x = f(x)` keeps `x` live-in correctly).
struct Node {
    /// Variables written here (full binds/assignments only — a partial write
    /// `x[i] = …` is a read-modify-write, so `x` is a *use*, not a killing def).
    defs: BTreeSet<String>,
    /// Variables read here.
    uses: BTreeSet<String>,
    /// True iff this node is a `clk.tick().await` (its out-edge is [`EdgeKind::Tick`]).
    is_tick: bool,
    /// The clock receiver identity of a tick node (e.g. `clk`) — item 4's domain tag.
    tick_clock: Option<String>,
    /// Successors and how control reaches them.
    succs: Vec<(usize, EdgeKind)>,
    /// Combinational output ports (`Out<…>`, not `RegOut`) driven here via
    /// `port.write(…)` — the def/use of *ports* the definite-assignment check keys on.
    writes: BTreeSet<String>,
    /// Memory-port access sites appearing in this node's own expression —
    /// `mem.read_port::<K>().read(a)` / `.data()` / `.is_ready()` and
    /// `mem.write_port::<K>().write(a, v)`. A *folded* nested loop node carries
    /// every access in its whole body (the region is opaque here; the sub-CFG in
    /// `nested_ticking_loops` checks its interior precisely).
    mem: Vec<MemAccess>,
    /// True iff this is a *folded* nested loop node whose `writes` is the
    /// conservative all-outputs over-approximation (not a real single `port.write`).
    /// The multi-write-collapse detector must not read it as an explicit write.
    folded: bool,
    /// True iff this is a folded **tick-bearing** nested loop that can nonetheless
    /// be left *without* ever reaching one of its ticks — a `loop`/`while` whose
    /// body can `break` (or fall out of a `while` test) before its first tick.
    ///
    /// The out-edge stays [`EdgeKind::Tick`], because the loop *does* cross a clock
    /// edge on its other paths and every value live across it must still be a
    /// register. This flag is read **only** by
    /// [`check_reachability`](Cfg::check_reachability), which must additionally
    /// consider the zero-tick path: an enclosing loop whose sole clock boundary is
    /// such a nested loop can cycle in zero time, and that is a livelock the
    /// simulator exhibits (measured) while the transpiled FSM would happily run one
    /// cycle per iteration.
    may_exit_without_tick: bool,
    /// Source span, for spanned diagnostics.
    span: Span,
}

impl Node {
    /// An empty node at `span`; combine with struct-update (`..Node::empty(span)`).
    fn empty(span: Span) -> Node {
        Node {
            defs: BTreeSet::new(),
            uses: BTreeSet::new(),
            is_tick: false,
            tick_clock: None,
            succs: Vec::new(),
            writes: BTreeSet::new(),
            mem: Vec::new(),
            folded: false,
            may_exit_without_tick: false,
            span,
        }
    }
}

/// The control-flow graph of a module's top-level `loop`.
pub struct Cfg {
    nodes: Vec<Node>,
    /// The loop-head node (empty; the DFS/liveness entry). Every "fall off the end
    /// of the body" and every trailing tick routes back here.
    head: usize,
    /// Register *candidates*: locals with a def site (let-binding or assignment)
    /// **inside** the loop. Pre-loop-only bindings (constants/wires like `lfsr`'s
    /// `xor_mask`) are excluded here, which is what stops them from being counted
    /// as registers even though they are live across ticks.
    defined_in_loop: BTreeSet<String>,
    /// The module's **combinational** output ports (`Out<…>`, excluding `RegOut`),
    /// from the signature — the ports the definite-assignment check requires to be
    /// driven on all paths (or none) per cycle.
    comb_outputs: BTreeSet<String>,
    /// The module's `In<…>` input ports, from the signature. Used by
    /// [`multi_write_collapse`](Self::multi_write_collapse) to spot a leading
    /// (deferred) port read before a straddling output write.
    inputs: BTreeSet<String>,
    /// The exit sink for a **combinational** body (loop-free): the single point all
    /// paths reach, where definite-assignment is checked. `None` for a sequential
    /// loop (definite-assignment does not apply — a sequential `Out` legitimately
    /// *holds* when unwritten, i.e. is an enabled register).
    exit: Option<usize>,
    /// Sub-CFGs of tick-containing nested loops, checked recursively by
    /// [`check_reachability`](Self::check_reachability). In the *parent* graph the
    /// nested loop stays a single conservatively-folded node (so its possible
    /// 0-iteration exit never false-rejects the outer loop); its own body's
    /// tickless-cycle well-formedness is enforced here instead.
    nested_ticking_loops: Vec<Cfg>,
}

impl Cfg {
    /// Build the CFG of `f`'s top-level `loop`, or `None` if `f` has no top-level
    /// loop (nothing sequential to analyze).
    pub fn build(f: &ItemFn) -> Option<Cfg> {
        let (loop_body, loop_span) = top_level_loop(f)?;

        let mut defined_in_loop = BTreeSet::new();
        let mut d = DefinedInLoop { set: &mut defined_in_loop };
        for stmt in &loop_body {
            d.visit_stmt(stmt);
        }

        let comb_outputs = combinational_outputs(f);

        let mut b = Builder::new(comb_outputs.clone(), in_param_names(f), memory_locals(f));
        // The head is node 0: an empty node whose successor is the first body node.
        // Every back-edge (trailing tick, fall-through) targets it.
        let head = b.new_node(Node::empty(loop_span));
        // A `continue` written directly in the hardware loop targets this head.
        b.top_head = Some(head);
        let body_entry = b.build_block(&loop_body, head);
        b.nodes[head].succs.push((body_entry, EdgeKind::Comb));

        Some(Cfg {
            nodes: b.nodes,
            head,
            defined_in_loop,
            comb_outputs,
            inputs: in_param_names(f),
            exit: None,
            nested_ticking_loops: b.nested,
        })
    }

    /// Build the CFG of a **combinational** module body (`#[hardware(combinational)]`
    /// — a loop-free `fn` whose outputs are pure combinational functions of its
    /// inputs). Unlike a sequential module there is no clock, no tick, and no state
    /// to hold, so an output left unassigned on some control path is a genuine
    /// **latch** — which [`check_definite_assignment`](Self::check_definite_assignment)
    /// rejects. Structure: `head → body → exit` sink, no back-edge.
    pub fn build_combinational(f: &ItemFn) -> Cfg {
        let comb_outputs = combinational_outputs(f);
        let mut b = Builder::new(comb_outputs.clone(), in_param_names(f), memory_locals(f));
        let span = f.sig.ident.span();
        let exit = b.new_node(Node::empty(span));
        let head = b.build_block(&f.block.stmts, exit);
        Cfg {
            nodes: b.nodes,
            head,
            defined_in_loop: BTreeSet::new(),
            comb_outputs,
            inputs: in_param_names(f),
            exit: Some(exit),
            nested_ticking_loops: b.nested,
        }
    }

    /// The inferred **register set** — sorted for stable structural comparison.
    ///
    /// A candidate (defined in the loop) is a register iff its value is live across
    /// some tick edge, i.e. it is in the live-out set of a tick node. Same-cycle
    /// combinational temps (redefined at the loop head before any post-tick use)
    /// are killed by that redefinition and correctly excluded.
    ///
    /// **Two clock boundaries, not one.** A local is a register iff it is defined in
    /// the loop **and** live across *either*
    ///
    ///   * a **tick edge** — its pre-tick value is read post-tick (the original
    ///     rule, which catches a register by its *use* on the far side of an edge); or
    ///   * the **loop back edge** — it is live entering the loop head, i.e. handed
    ///     from one iteration to the next.
    ///
    /// The back-edge clause was added 2026-08-21 to fix a real under-approximation
    /// (see the `sync_2ff` case below). It is not redundant with the tick clause:
    /// anything passed from one *pre*-tick segment to the next must pass through the
    /// intervening tick and is already caught, so the only values it newly admits are
    /// those **defined in a post-tick segment and read before the next tick**.
    ///
    /// Those are genuine flip-flops, and the reason is the *ordering* inside the
    /// post-tick segment. In the 2-FF synchronizer (`src/sync.rs`):
    ///
    /// ```text
    /// loop { q.write(ff2); clk.tick().await; ff2 = ff1; ff1 = d.read(); }
    /// ```
    ///
    /// `ff2` is used pre-tick and defined post-tick, so walking backwards from the
    /// use, its own definition kills it before the tick is reached — under the tick
    /// clause alone it looks like a same-cycle wire, and inference reported **one**
    /// register where the simulator's behaviour, an independent hand-written
    /// reference, and codegen all have **two**. But `ff2 = ff1` reads `ff1`'s
    /// *pre-edge* value (the next line overwrites it), which no wire can reproduce:
    /// `assign ff2 = ff1` would track `ff1`'s post-edge value and collapse the two
    /// stages into one flop — the exact failure `src/sync.rs`'s own comment warns
    /// about. Only an edge-triggered, non-blocking update behaves as observed.
    ///
    /// Generally: a local defined in a post-tick segment had its defining expression
    /// evaluated against *pre-edge* values, while its consumer runs after the other
    /// post-edge updates — so if it survives to the next iteration it needs storage.
    /// A post-tick temp that dies within its own segment is not live at the head and
    /// is still correctly excluded (it is combinational logic in a D-input path).
    pub fn registers(&self) -> Vec<String> {
        let live_out = self.liveness();
        let mut across_boundary = BTreeSet::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if node.is_tick {
                across_boundary.extend(live_out[i].iter().cloned());
            }
        }
        // The head is an empty node (no defs/uses), so its live-out is exactly the
        // set live entering it — i.e. carried across the back edge.
        across_boundary.extend(live_out[self.head].iter().cloned());

        self.defined_in_loop
            .intersection(&across_boundary)
            .cloned()
            .collect()
    }

    /// Combinational output ports (`Out<…>`, not `RegOut`) that hit the
    /// **multi-write-around-a-tick collapse**: written on both sides of a *bare*
    /// `clk.tick().await` within one iteration, where the pre-tick write is
    /// phase-shifted into the pre-edge by a *leading (deferred) input read*. The
    /// coroutine simulator then runs the post-tick write in the same `tick_clock`
    /// and clobbers the pre-tick value before it is observed (silent sim ≠ synth).
    /// Returns the offending ports, sorted, for a macro guardrail to reject.
    ///
    /// Precise by construction (validated empirically against the corpus): the three
    /// necessary conditions are all required, so designs that merely straddle a tick
    /// without the collapsing alignment are not flagged —
    ///   * a *bare* tick (`is_tick` with no writes), not a folded multi-tick loop
    ///     (whose ticks separate the writes into distinct `tick_clock`s — `uart_tx`);
    ///   * the same `Out` written on both sides of that bare tick in one iteration;
    ///   * a leading `In` read that comb-reaches the pre-tick write (what pushes it to
    ///     the pre-edge). Its absence is why `counter` and `uart_rx`'s `rx_dv`
    ///     (`write(1); tick; write(0)` with no leading read) do **not** collapse.
    /// `RegOut` outputs are excluded by construction (they are not in `comb_outputs`).
    ///
    /// Recurses into nested ticking loops (`for`/`while` that await a tick): a
    /// straddle *inside* such a loop is caught in its sub-CFG, which carries the same
    /// port sets. So both a top-level and a nested-loop collapse are reported.
    pub fn multi_write_collapse(&self) -> Vec<String> {
        let mut flagged: BTreeSet<String> = self.multi_write_collapse_local().into_iter().collect();
        for sub in &self.nested_ticking_loops {
            flagged.extend(sub.multi_write_collapse());
        }
        flagged.into_iter().collect()
    }

    /// Collapse detection over this CFG's own nodes only (top-level of one loop);
    /// [`multi_write_collapse`](Self::multi_write_collapse) adds the nested recursion.
    fn multi_write_collapse_local(&self) -> Vec<String> {
        let mut flagged = BTreeSet::new();
        for (t, node) in self.nodes.iter().enumerate() {
            if !(node.is_tick && node.writes.is_empty()) {
                continue; // only a bare `clk.tick().await`
            }
            let after = self.post_tick_writes(t);
            for p in &self.comb_outputs {
                if flagged.contains(p) || !after.contains(p) {
                    continue;
                }
                // A pre-tick write of `p` that this bare tick ends, with a leading
                // input read reaching it. (Skip folded loops: their `writes` is a
                // conservative all-outputs over-approximation, not a real write.)
                for w1 in 0..self.nodes.len() {
                    if self.nodes[w1].is_tick
                        || self.nodes[w1].folded
                        || !self.nodes[w1].writes.contains(p)
                    {
                        continue;
                    }
                    if self.comb_reaches(w1, t) && self.leading_read_reaches(w1) {
                        flagged.insert(p.clone());
                        break;
                    }
                }
            }
        }
        flagged.into_iter().collect()
    }

    /// Ports explicitly written in the cycle-region *after* bare tick `t` (comb-
    /// reachable from its tick-successor, within the same iteration).
    fn post_tick_writes(&self, t: usize) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let Some(&(start, _)) = self.nodes[t].succs.iter().find(|(_, k)| *k == EdgeKind::Tick)
        else {
            return out;
        };
        let mut stack = vec![start];
        let mut seen = vec![false; self.nodes.len()];
        while let Some(n) = stack.pop() {
            if n == self.head || std::mem::replace(&mut seen[n], true) {
                continue;
            }
            if !self.nodes[n].is_tick && !self.nodes[n].folded {
                out.extend(self.nodes[n].writes.iter().cloned());
            }
            for &(s, k) in &self.nodes[n].succs {
                if k == EdgeKind::Comb {
                    stack.push(s);
                }
            }
        }
        out
    }

    /// True iff `b` is reachable from `a` via comb-only edges (same cycle-region),
    /// not crossing a tick or the loop head.
    fn comb_reaches(&self, a: usize, b: usize) -> bool {
        let mut stack = vec![a];
        let mut seen = vec![false; self.nodes.len()];
        while let Some(n) = stack.pop() {
            if n == b {
                return true;
            }
            if n == self.head || std::mem::replace(&mut seen[n], true) {
                continue;
            }
            for &(s, k) in &self.nodes[n].succs {
                if k == EdgeKind::Comb {
                    stack.push(s);
                }
            }
        }
        false
    }

    /// True iff some node reading an `In` port comb-reaches `w1` (i.e. a leading
    /// input read sits before the write in its region — including `w1`'s own reads).
    fn leading_read_reaches(&self, w1: usize) -> bool {
        (0..self.nodes.len()).any(|r| {
            !self.nodes[r].uses.is_disjoint(&self.inputs) && self.comb_reaches(r, w1)
        })
    }

    /// Nodes in the **pre-tick region**: reachable from the loop head over `Comb`
    /// edges only, so traversal stops at every tick. This is the segment whose phase
    /// alignment is decided incidentally — see
    /// [`unprotected_pretick_out_write`](Self::unprotected_pretick_out_write).
    fn pre_tick_region(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen = vec![false; self.nodes.len()];
        let mut stack: Vec<usize> = self.nodes[self.head]
            .succs
            .iter()
            .filter(|(_, k)| *k == EdgeKind::Comb)
            .map(|(s, _)| *s)
            .collect();
        while let Some(n) = stack.pop() {
            if n == self.head || std::mem::replace(&mut seen[n], true) {
                continue;
            }
            out.push(n);
            for &(s, k) in &self.nodes[n].succs {
                if k == EdgeKind::Comb {
                    stack.push(s);
                }
            }
        }
        out
    }

    /// Combinational `Out` ports whose value is exposed to the **pre-tick alignment
    /// hazard**: the pre-tick segment assigns a register on a path with **no `In`
    /// read preceding it**, and drives a plain `Out` in that same segment. Returns
    /// the offending ports, sorted; empty for a combinational module, a module with
    /// no top-level loop, or one whose outputs are all `RegOut`.
    ///
    /// # The hazard
    ///
    /// A leading `In` read classifies `Deferred` (impl-plan item 3) and injects
    /// `pre_edge_barrier()`, which parks the task at the barrier so the pre-tick
    /// segment runs in the **pre-edge** phase. With no such read the task parks at the
    /// tick instead, and the segment for cycle *N+1* runs during cycle *N*'s
    /// **post-edge settle** — so the post-edge observation of cycle *N* sees *N+1*'s
    /// value. Codegen emits a non-blocking `r <= …`, which cannot reproduce that.
    /// Measured: `loop { r = r+1; o.write(r); tick; }` simulates `[2,3,4,…]` against
    /// the SV's `[1,2,3,…]`.
    ///
    /// # Why it keys on the `Out` write, not on the register
    ///
    /// **`RegOut` is immune by construction** — it buffers and commits at the edge, so
    /// the phase at which the write executes cannot be observed. This was established
    /// by changing *only* the port type on otherwise-identical modules: both the
    /// minimal case and a mixed-alignment case flip from diverging to agreeing. An
    /// earlier rule keyed on registers and therefore rejected `mac_fsm`,
    /// `if_tick_explicit` and `branch_merge_explicit` — all correct `RegOut` designs.
    /// `Node::writes` already holds only combinational outputs, so `RegOut` is
    /// excluded here for free, exactly as it is in
    /// [`multi_write_collapse`](Self::multi_write_collapse).
    ///
    /// # Why the write must read a register
    ///
    /// The misalignment changes *when* the write happens, so it is observable only if
    /// the value written differs between the two phases. A write of a **constant** is
    /// idempotent across the shift — measured on `branch_merge_explicit`, which drives
    /// three plain `Out`s from an unprotected path and **agrees** with its transpiled
    /// SV because every write is `Logic::One`.
    ///
    /// # Why the read must *precede* the assignment
    ///
    /// The barrier suspends at the point the read appears, so a read placed *after*
    /// the assignment does not protect it — measured, and the reason this uses
    /// `leading_read_reaches` (a comb-path query) rather than "the module reads an
    /// input somewhere". Mixed alignment does **not** protect either: a module with a
    /// read on one branch and an unprotected assignment on another still diverges.
    ///
    /// # Known false negative
    ///
    /// Only the pre-tick segment is examined, so a hazard in a *middle* segment of a
    /// multi-tick loop would not be caught. This is **theoretical** — the case
    /// originally cited for it, `accum_2`, was measured and does not diverge (its
    /// `#[ignore]` was stale; it is un-ignored now). There is no known instance;
    /// measure one before widening the rule. Tracked as Q5 in
    /// `design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`.
    pub fn unprotected_pretick_out_write(&self) -> Vec<String> {
        let region = self.pre_tick_region();

        let regs: BTreeSet<String> = self.registers().into_iter().collect();

        // (ii) plain combinational `Out` ports whose VALUE differs between the two
        // phases, which is the only way the misalignment becomes observable. Two ways
        // that happens, and the second was missing until 2026-08-25:
        //
        //   * the port is driven **from a register**, so the value depends on when
        //     the write runs relative to the update; or
        //   * the port is driven **conditionally** — some path through the segment
        //     leaves it unwritten, so it HOLDS there. A write of a constant is
        //     idempotent across the phase shift only if it happens on every path;
        //     where the alternative is the held value, *when* the write lands is
        //     observable even though the value written never changes.
        //
        // The second clause is `pc_arm_write` in sequential_forwarding_divergence.rs
        // (5.5 of the guardrail): a `match pc` whose arms write a constant or nothing
        // at all, measured leading its own emitted SystemVerilog by one cycle.
        //
        // A `folded` node carries the conservative all-outputs over-approximation for
        // a nested loop, not a real `port.write`, so it is not evidence either way.
        let mut driven: BTreeSet<String> = BTreeSet::new();
        for &n in &region {
            let node = &self.nodes[n];
            if node.folded || node.writes.is_empty() {
                continue;
            }
            for w in &node.writes {
                if !node.uses.is_disjoint(&regs) || !self.written_on_all_paths(w) {
                    driven.insert(w.clone());
                }
            }
        }
        if driven.is_empty() {
            return Vec::new();
        }

        // (i) a register assigned on a path no leading `In` read reaches.
        let unprotected = region.iter().any(|&n| {
            !self.nodes[n].defs.is_disjoint(&regs) && !self.leading_read_reaches(n)
        });

        if unprotected { driven.into_iter().collect() } else { Vec::new() }
    }

    /// Does every path from the loop head to a clock edge drive `port`?
    ///
    /// A port left unwritten on some path HOLDS its previous value there — the
    /// enabled-register idiom, which is legitimate — and that is exactly what makes
    /// the pre-tick phase shift observable for a constant write: the port's value
    /// then depends on WHEN the write ran, not only on what it wrote.
    ///
    /// The region is acyclic (a tickless cycle is rejected by `check_reachability`),
    /// so the memoized recursion terminates. A path that reaches a tick, or returns
    /// to the head, without writing counts as not-written.
    fn written_on_all_paths(&self, port: &str) -> bool {
        fn go(
            cfg: &Cfg,
            n: usize,
            port: &str,
            memo: &mut std::collections::HashMap<usize, bool>,
        ) -> bool {
            if let Some(v) = memo.get(&n) {
                return *v;
            }
            let node = &cfg.nodes[n];
            if !node.folded && node.writes.contains(port) {
                memo.insert(n, true);
                return true;
            }
            // Guard against a cycle the reachability check would have rejected
            // anyway: assume not-written while this node is in progress.
            memo.insert(n, false);
            let comb: Vec<usize> = node
                .succs
                .iter()
                .filter(|(s, k)| *k == EdgeKind::Comb && *s != cfg.head)
                .map(|&(s, _)| s)
                .collect();
            let all = !comb.is_empty() && comb.iter().all(|&s| go(cfg, s, port, memo));
            memo.insert(n, all);
            all
        }

        let mut memo = std::collections::HashMap::new();
        let entries: Vec<usize> = self.nodes[self.head]
            .succs
            .iter()
            .filter(|(_, k)| *k == EdgeKind::Comb)
            .map(|&(s, _)| s)
            .collect();
        !entries.is_empty() && entries.iter().all(|&e| go(self, e, port, &mut memo))
    }

    /// Plain combinational `Out` ports driven in **more than one clock phase**.
    ///
    /// The multi-tick lowering already refuses this — *"output port `p` is driven in
    /// more than one phase (across `clk.tick().await` boundaries), which would emit
    /// multiple conflicting drivers. Drive it in exactly one phase, or hold it in a
    /// register"* — but only when it can see the phases. **Control extraction hides
    /// them**: it rewrites a body whose ticks live inside branches or loops into a
    /// single-tick `match pc { … }` FSM, so the check counts one tick and passes,
    /// while the `pc` states are exactly the phases it was meant to count.
    ///
    /// The consequence is measured, not argued. A one-cycle pulse —
    ///
    /// ```text
    /// loop { for _ in 0..3 { tick } dv.write(One); tick; dv.write(Zero); }
    /// ```
    ///
    /// — diverges from its transpiled SystemVerilog by exactly one cycle, uniformly,
    /// because with no barrier the simulator runs a segment during the PREVIOUS
    /// cycle's post-edge settle while the FSM gives it its own state. That is the
    /// pre-tick alignment family (D1), and `RegOut` is immune to it by construction:
    /// the phase at which the write executes is unobservable through a port that
    /// commits at the edge. Replacing the port type is the whole fix, which is why
    /// the diagnostic points there — the same remedy `multi_write_collapse` and the
    /// D1 guardrail already point at.
    ///
    /// # Why not widen the D1 rule instead
    ///
    /// Tried and measured. `unprotected_pretick_out_write` examines only head →
    /// first tick (plan Q5). Widening it to every post-tick segment flags **36 of
    /// 120** corpus modules, ~30 of which have passing equivalence tests —
    /// `det_010`, `mac_pipeline`, `dual_port_ram`, `bsg_dff_en`, every memory
    /// fixture. Writing a plain `Out` after a tick is the ORDINARY multi-phase
    /// pattern and is correct; writing it in *two* phases is not. This rule flags
    /// 9 modules on the same corpus, six of them the synthetic witnesses that
    /// establish it.
    ///
    /// # What a "phase" is here
    ///
    /// A Comb-connected component of the CFG. A tick is the only edge that separates
    /// two of them, and the trailing segment merges with the head — correctly, since
    /// falling off the end of the body and re-entering it costs no cycle, so they
    /// run in the same one.
    ///
    /// `RegOut` is excluded for free: `Node::writes` holds only combinational
    /// outputs, the same way `multi_write_collapse` gets its exclusion.
    pub fn multi_phase_out_write(&self) -> Vec<String> {
        let n = self.nodes.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut Vec<usize>, mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for i in 0..n {
            let succs: Vec<usize> = self.nodes[i]
                .succs
                .iter()
                .filter(|(_, k)| *k == EdgeKind::Comb)
                .map(|&(s, _)| s)
                .collect();
            for s in succs {
                let (a, b) = (find(&mut parent, i), find(&mut parent, s));
                if a != b {
                    parent[a] = b;
                }
            }
        }
        let mut phases: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
        for i in 0..n {
            // A `folded` node carries the conservative all-outputs over-approximation
            // for a nested loop, not a real `port.write` — counting it as evidence
            // would flag every output of every module with a nested loop.
            if self.nodes[i].folded || self.nodes[i].writes.is_empty() {
                continue;
            }
            let r = find(&mut parent, i);
            for w in &self.nodes[i].writes {
                phases.entry(w.clone()).or_default().insert(r);
            }
        }
        phases.into_iter().filter(|(_, p)| p.len() > 1).map(|(port, _)| port).collect()
    }

    /// Enforce the memory-port rules **on the source**, where the clock edges are
    /// still visible.
    ///
    /// Copper's memory model is that `read`/`write` *stage* a bus during a cycle and
    /// `data`/`is_ready` *observe* what the following clock edge produced. Three
    /// things follow, and each guards a place where the emitted array would silently
    /// disagree with the simulator:
    ///
    /// * **One access per bus per cycle.** A physical port has one address bus. The
    ///   simulator silently keeps the last write of a cycle; hardware cannot.
    /// * **A result is observed after the edge that produces it** — never in the same
    ///   cycle as the staging that feeds it, which would read it a cycle early.
    /// * **A result has a staging somewhere.** Observing a port nothing ever stages
    ///   reads a port that never becomes ready.
    ///
    /// # Why this is not in codegen
    ///
    /// It was, in `chir_lower::validate_memory_usage`, expressed over the *segments*
    /// a loop body splits into at `clk.tick().await`. `control_extract` runs first
    /// and rewrites any body whose ticks live inside branches or loops into a
    /// **single-tick `match pc` FSM** — so the segments it counted are gone, and
    /// every access in the module lands in one of them. Measured: this transpiles
    ///
    /// ```text
    /// loop { rom.read(addr); tick; data.write(rom.data()); }
    /// ```
    ///
    /// and appending `for _ in 0..2 { tick }` — which changes nothing about the
    /// staging — made it fail with *"read before the `clk.tick().await` that produces
    /// it"*. A false positive that made **any** memory design needing control
    /// extraction unwritable. Same class as [`multi_phase_out_write`](Self::multi_phase_out_write):
    /// a check that counts a syntactic feature placed downstream of a pass that
    /// legitimately removes it.
    ///
    /// # What replaces "segment k precedes segment n"
    ///
    /// Reachability over **tick-free** paths, which is the thing segment order was
    /// approximating. An observation is early iff *every* staging of its port can
    /// reach it without crossing a tick edge, and two accesses share a bus cycle iff
    /// one reaches the other the same way. Walking through the loop head is
    /// deliberate: falling off the end of the body and re-entering it costs no
    /// cycle, so the trailing statements and the head run in the same one — which is
    /// also why the segment-index form got that pairing wrong in both directions.
    ///
    /// A **folded** nested loop is opaque here, so its accesses are attributed to the
    /// node as a whole (enough for "a staging exists" and for the tick edge that
    /// separates it from what follows) and its interior is checked precisely in its
    /// own sub-CFG, which is where the ticks inside it are visible.
    pub fn check_memory_staging(&self) -> Result<(), (Span, String)> {
        self.check_memory_staging_inner(true)
    }

    /// `require_staging` is false inside a nested loop's sub-CFG: the staging that
    /// feeds an observation there may well live in the enclosing loop, so only the
    /// *ordering* rules apply. The enclosing CFG sees every access (the folded node
    /// carries the whole body) and answers the existence question for both.
    fn check_memory_staging_inner(&self, require_staging: bool) -> Result<(), (Span, String)> {
        // A tick-free nested loop is unrolled, so an access inside it becomes N
        // accesses on one bus in one cycle. Refused rather than counted as one.
        for node in &self.nodes {
            if node.folded && !node.is_tick && !node.mem.is_empty() {
                let a = &node.mem[0];
                return Err((
                    node.span,
                    format!(
                        "memory `{}` is accessed inside a nested loop with no \
                         `clk.tick().await`; that loop is unrolled, so every iteration's \
                         access lands in the same cycle on one address bus",
                        a.mem
                    ),
                ));
            }
        }

        // One access per bus per cycle. Two accesses conflict iff one can reach the
        // other WITHOUT crossing a clock edge — not merely iff they share a phase.
        // The distinction is the whole difference between a bus conflict and a
        // multiplexer: `rv32i_cpu`'s seven regfile writebacks sit in exclusive
        // `match` arms, no path joins any two of them, and each drives the bus in
        // its own state — which is exactly what the emitted `always_comb` does.
        // Counting them instead reported a design error where there is a mux.
        //
        // A folded tick-bearing loop contributes at most ONE site per bus: its
        // interior may legitimately access the bus once per iteration (separate
        // cycles), and its own sub-CFG is where that is checked. It does still
        // contribute, because its first iteration runs in the cycle that reaches it.
        let mut sites: Vec<(usize, (String, bool, String), Span)> = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            let mut here: BTreeSet<(String, bool, String)> = BTreeSet::new();
            for a in &node.mem {
                let Some(is_read) = a.kind.bus() else { continue };
                let key = (a.mem.clone(), is_read, a.port.clone());
                if node.folded && !here.insert(key.clone()) {
                    continue;
                }
                sites.push((i, key, node.span));
            }
        }
        for (p, (i, key, span)) in sites.iter().enumerate() {
            let conflicting: Vec<&(usize, (String, bool, String), Span)> = sites
                .iter()
                .enumerate()
                .filter(|(q, (j, k, _))| {
                    *q != p
                        && k == key
                        && (self.tick_free_reaches(*i, *j) || self.tick_free_reaches(*j, *i))
                })
                .map(|(_, s)| s)
                .collect();
            if conflicting.is_empty() {
                continue;
            }
            let (mem, is_read, port) = key;
            let n = conflicting.len() + 1;
            return Err((
                *span,
                format!(
                    "memory `{mem}` {} port {port} is accessed {n} times in one cycle; a \
                     physical port has a single address bus. Merge the accesses into one, \
                     selecting the address and data with a conditional",
                    if *is_read { "read" } else { "write" }
                ),
            ));
        }

        // Where each read port is staged, so an observation can ask whether any
        // staging is separated from it by a clock edge. Positions come from THIS
        // graph; existence (below) looks through the folds as well, since a staging
        // inside a nested loop is still a staging.
        let mut staged: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
        for (i, node) in self.nodes.iter().enumerate() {
            for a in &node.mem {
                if a.kind == MemAccessKind::StageRead {
                    staged.entry((a.mem.clone(), a.port.clone())).or_default().push(i);
                }
            }
        }
        let anywhere = self.staged_ports();

        for (i, node) in self.nodes.iter().enumerate() {
            for a in &node.mem {
                let MemAccessKind::Observe(what) = a.kind else { continue };
                let (mem, port) = (&a.mem, &a.port);
                let Some(sites) = staged.get(&(mem.clone(), port.clone())) else {
                    if !require_staging || anywhere.contains(&(mem.clone(), port.clone())) {
                        continue;
                    }
                    return Err((
                        node.span,
                        format!(
                            "`{mem}.read_port::<{port}>().{what}()` is read, but nothing stages a \
                             `read()` on that port earlier in the loop — the port never becomes \
                             ready. Stage the address with `read(addr)` before a \
                             `clk.tick().await`, and observe the result after it"
                        ),
                    ));
                };
                // A staging the observation is NOT tick-free-reachable from is one
                // whose result an edge has already produced — the shape the rule wants.
                if sites.iter().any(|&s| !self.tick_free_reaches(s, i)) {
                    continue;
                }
                return Err((
                    node.span,
                    format!(
                        "`{mem}.read_port::<{port}>().{what}()` is read before the \
                         `clk.tick().await` that produces it. The read result appears at the \
                         clock edge, so it must be observed after the tick — stage the address \
                         with `read(addr)` before a `clk.tick().await`, and observe the result \
                         after it"
                    ),
                ));
            }
        }

        for sub in &self.nested_ticking_loops {
            sub.check_memory_staging_inner(false)?;
        }
        Ok(())
    }

    /// Plain combinational `Out` ports driven from a **memory read result** in a
    /// module with more than one clock phase.
    ///
    /// The read pipeline re-captures on every clock edge, so a plain `Out` wired to
    /// it either tracks it into the phases that do not observe it, or — once the
    /// implicit-hold conversion takes over — latches one edge after the capture the
    /// simulator reads. Measured: a full cycle late on every sampled value. `RegOut`
    /// is the form that works, or a register between the result and the port.
    ///
    /// `vlir_lower::reject_memory_driven_comb_outputs` states the same rule over the
    /// lowered phases, and cannot see this case: it gives up on `phases.len() < 2`,
    /// and an extracted module has exactly one lowered phase no matter how many
    /// clock phases the source has. That blind spot was harmless only for as long as
    /// the memory staging rules happened to reject every extracted memory design
    /// before it could matter — measured the day they stopped:
    /// `loop { rom.read(a); tick; o.write(rom.data()); for _ in 0..2 { tick } }`
    /// diverges by exactly one cycle, uniformly. Hence this copy, on the source,
    /// where the phases are still countable.
    ///
    /// A **single-phase** module is unaffected and deliberately so: there the
    /// post-tick segment shares the head's phase and a plain `Out` driven from
    /// `data()` is correct (`rom_direct`, pinned in
    /// `tests/multiphase_memory_equivalence.rs`).
    ///
    /// The drive is followed through same-cycle locals: a value that survives a
    /// clock edge is a register by construction, so excluding registers from the
    /// taint is what keeps it within one phase.
    pub fn memory_result_drives_plain_out(&self) -> Vec<String> {
        let phases = self.comb_phases();
        if phases.iter().collect::<BTreeSet<_>>().len() < 2 {
            return Vec::new();
        }
        let observes = |n: &Node| n.mem.iter().any(|a| matches!(a.kind, MemAccessKind::Observe(_)));
        if !self.nodes.iter().any(observes) {
            return Vec::new();
        }

        let regs: BTreeSet<String> = self.registers().into_iter().collect();
        let mut tainted: BTreeSet<String> = BTreeSet::new();
        loop {
            let mut changed = false;
            for node in &self.nodes {
                if !(observes(node) || node.uses.iter().any(|u| tainted.contains(u))) {
                    continue;
                }
                for d in &node.defs {
                    if !regs.contains(d) && tainted.insert(d.clone()) {
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        let mut out = BTreeSet::new();
        for node in &self.nodes {
            // A folded node's `writes` is the conservative all-outputs
            // over-approximation, not a real `port.write`.
            if node.folded || node.writes.is_empty() {
                continue;
            }
            if observes(node) || node.uses.iter().any(|u| tainted.contains(u)) {
                out.extend(node.writes.iter().cloned());
            }
        }
        out.into_iter().collect()
    }

    /// How many **clock phases** the source has: the number of Comb-connected
    /// components reachable in the loop, i.e. the number of distinct cycles one
    /// iteration of the body occupies.
    ///
    /// This is the same notion `shir_lower` re-derives downstream by splitting the
    /// LOWERED body at `clk.tick().await` — and the two disagree whenever a pass
    /// between them changes the tick structure, which `control_extract` does by
    /// design. Exposed so that disagreement can be measured rather than argued
    /// about; see `design_docs/TIMING_MODEL_UNIFICATION.md`.
    pub fn clock_phase_count(&self) -> usize {
        let phases = self.comb_phases();
        let reachable: BTreeSet<usize> = (0..self.nodes.len())
            .filter(|&i| self.reaches_from_head(i))
            .map(|i| phases[i])
            .collect();
        reachable.len()
    }

    /// Is `n` reachable from the loop head at all? An unreachable node's phase must
    /// not be counted — `break` sinks in a nested sub-CFG, for instance.
    fn reaches_from_head(&self, n: usize) -> bool {
        let mut stack = vec![self.head];
        let mut seen = vec![false; self.nodes.len()];
        while let Some(x) = stack.pop() {
            if x == n {
                return true;
            }
            if std::mem::replace(&mut seen[x], true) {
                continue;
            }
            for &(s, _) in &self.nodes[x].succs {
                stack.push(s);
            }
        }
        false
    }

    /// Union-find over `Comb` edges: `phases[n]` is the representative of the clock
    /// phase (Comb-connected component) node `n` runs in. A tick edge is the only
    /// thing that separates two of them, and the trailing statements merge with the
    /// head — re-entering the body costs no cycle.
    fn comb_phases(&self) -> Vec<usize> {
        let n = self.nodes.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut Vec<usize>, mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for i in 0..n {
            let succs: Vec<usize> = self.nodes[i]
                .succs
                .iter()
                .filter(|(_, k)| *k == EdgeKind::Comb)
                .map(|&(s, _)| s)
                .collect();
            for s in succs {
                let (a, b) = (find(&mut parent, i), find(&mut parent, s));
                if a != b {
                    parent[a] = b;
                }
            }
        }
        (0..n).map(|i| find(&mut parent, i)).collect()
    }

    /// The accesses that run in the cycle this (nested-loop) CFG is *entered* in —
    /// those reachable from the head without crossing a tick edge. The enclosing
    /// graph folds this loop into one node and needs exactly these: the rest of the
    /// body is separated from that node's cycle by the loop's own clock edges.
    fn entry_cycle_mem(&self) -> Vec<MemAccess> {
        let mut out = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if !node.mem.is_empty() && self.tick_free_reaches(self.head, i) {
                out.extend(node.mem.iter().cloned());
            }
        }
        out
    }

    /// Every read port staged **anywhere** in this loop, nested loops included.
    /// Existence does not depend on position, so unlike the ordering rules this one
    /// looks straight through the folds.
    fn staged_ports(&self) -> BTreeSet<(String, String)> {
        let mut out: BTreeSet<(String, String)> = self
            .nodes
            .iter()
            .flat_map(|n| n.mem.iter())
            .filter(|a| a.kind == MemAccessKind::StageRead)
            .map(|a| (a.mem.clone(), a.port.clone()))
            .collect();
        for sub in &self.nested_ticking_loops {
            out.extend(sub.staged_ports());
        }
        out
    }

    /// Is there a path from `a` to `b` that crosses no clock edge — i.e. do they run
    /// in the same cycle, with `a` before `b`? `a == b` counts (one statement, one
    /// cycle). Unlike [`comb_reaches`](Self::comb_reaches) this walks **through** the
    /// loop head, because re-entering the body costs no cycle.
    fn tick_free_reaches(&self, a: usize, b: usize) -> bool {
        let mut stack = vec![a];
        let mut seen = vec![false; self.nodes.len()];
        while let Some(n) = stack.pop() {
            if n == b {
                return true;
            }
            if std::mem::replace(&mut seen[n], true) {
                continue;
            }
            for &(s, k) in &self.nodes[n].succs {
                if k == EdgeKind::Comb {
                    stack.push(s);
                }
            }
        }
        false
    }

    /// Enforce the reachability invariant: with every tick edge deleted, the graph
    /// reachable from the loop head must be acyclic. A remaining cycle is a path
    /// that returns to the top of the loop without ever awaiting a clock tick — a
    /// zero-time combinational loop. Returns the span of the offending node and a
    /// message on violation.
    pub fn check_reachability(&self) -> Result<(), (Span, String)> {
        #[derive(Clone, Copy, PartialEq)]
        enum Color {
            White,
            Gray,
            Black,
        }
        let mut color = vec![Color::White; self.nodes.len()];

        // Iterative DFS (avoid recursion depth limits on large bodies). The stack
        // holds (node, next-successor-index); a Gray successor over a zero-time edge
        // is a back-edge — a tickless cycle.
        //
        // A zero-time edge is a Comb edge, OR the Tick edge of a folded nested loop
        // that can be left before it ever ticks (`may_exit_without_tick`): that
        // loop's zero-tick exit is a real path through the parent, even though the
        // edge stays Tick for liveness.
        let mut stack: Vec<(usize, usize)> = vec![(self.head, 0)];
        color[self.head] = Color::Gray;
        while let Some(&mut (n, ref mut si)) = stack.last_mut() {
            // Advance to the next zero-time successor of n.
            let mut advanced = false;
            while *si < self.nodes[n].succs.len() {
                let (s, kind) = self.nodes[n].succs[*si];
                *si += 1;
                if kind != EdgeKind::Comb && !self.nodes[n].may_exit_without_tick {
                    continue;
                }
                match color[s] {
                    Color::Gray => {
                        // Name the nested loop when one is on the cycle: "add a tick"
                        // is unfollowable advice for a path that already has ticks on
                        // its *other* branches, and the fix is a different one.
                        let culprit = stack
                            .iter()
                            .position(|&(m, _)| m == s)
                            .map(|start| &stack[start..])
                            .unwrap_or(&stack[..])
                            .iter()
                            .find(|&&(m, _)| self.nodes[m].may_exit_without_tick);
                        if let Some(&(m, _)) = culprit {
                            return Err((
                                self.nodes[m].span,
                                "this loop's only clock boundary is a nested loop that can be \
                                 left before it ticks, so the enclosing loop can return to its \
                                 top in zero time — a livelock in simulation, and a loop the \
                                 transpiler cannot give the same meaning in hardware. Give the \
                                 enclosing loop a `clk.tick().await` of its own, or make the \
                                 nested loop one that always ticks (a `for` over a constant \
                                 range, whose body runs before any exit test)"
                                    .to_string(),
                            ));
                        }
                        return Err((
                            self.nodes[n].span,
                            "this control-flow path returns to the top of the loop without \
                             awaiting a clock tick — every path through a `#[hardware]` loop must \
                             reach `clk.tick().await`, otherwise it is a zero-time combinational \
                             cycle"
                                .to_string(),
                        ));
                    }
                    Color::White => {
                        color[s] = Color::Gray;
                        stack.push((s, 0));
                        advanced = true;
                        break;
                    }
                    Color::Black => {}
                }
            }
            if !advanced && stack.last().is_some_and(|&(m, _)| m == n) {
                // Exhausted n's successors.
                color[n] = Color::Black;
                stack.pop();
            }
        }
        // Recurse into each tick-containing nested loop: its body must satisfy the
        // same tickless-cycle invariant (a clocked nested loop whose body can cycle
        // without ticking spins forever). Folded conservatively in *this* graph, so
        // this recursion is where nested-loop malformedness is actually caught.
        for sub in &self.nested_ticking_loops {
            sub.check_reachability()?;
        }
        Ok(())
    }

    /// Enforce **definite assignment** for a **combinational** module's outputs: a
    /// combinational `Out` port must be driven on *all* control paths or *none* —
    /// never some-but-not-all. A partial (conditional) write in a combinational body
    /// infers a **latch** (there is no clock to register it and no prior value to
    /// legitimately hold). This mirrors, at the syntax level and in the sim macro,
    /// the transpiler's own late `check_no_latches` (`vlir_lower.rs`, `any − all`) —
    /// so the sim rejects the latch at compile time instead of only at transpile.
    ///
    /// **Only applies to combinational bodies** (built via [`build_combinational`],
    /// so `exit` is `Some`). For a sequential loop (`exit == None`) this is a no-op:
    /// a sequential `Out` legitimately holds when unwritten (an *enabled register* —
    /// verified `sim ≡ BaseJump` on `bsg_dff_en`), so "assign on all paths" must not
    /// be imposed there.
    ///
    /// Criterion (`MAY − MUST` at the exit): an output written on some path to the
    /// body's exit but not all was partially assigned. Folded nested loops are
    /// treated as driving every output (opaque, never a *false* latch). Outputs
    /// *never* written are not flagged (they may be driven by a submodule).
    ///
    /// [`build_combinational`]: Self::build_combinational
    pub fn check_definite_assignment(&self) -> Result<(), (Span, String)> {
        let Some(exit) = self.exit else {
            return Ok(()); // sequential: `Out` holds when unwritten — not a latch.
        };
        if self.comb_outputs.is_empty() {
            return Ok(());
        }
        let n = self.nodes.len();
        let mut preds: Vec<Vec<(usize, EdgeKind)>> = vec![Vec::new(); n];
        for (i, node) in self.nodes.iter().enumerate() {
            for &(s, kind) in &node.succs {
                preds[s].push((i, kind));
            }
        }

        // MAY-write (union; a tick edge contributes nothing — new cycle).
        let mut may: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
        loop {
            let mut changed = false;
            for i in 0..n {
                let mut out = BTreeSet::new();
                for &(p, kind) in &preds[i] {
                    if kind != EdgeKind::Tick {
                        out.extend(may[p].iter().cloned());
                    }
                }
                out.extend(self.nodes[i].writes.iter().cloned());
                if out != may[i] {
                    may[i] = out;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // MUST-write (intersection; a tick edge contributes ∅; init to TOP).
        let mut must: Vec<BTreeSet<String>> = vec![self.comb_outputs.clone(); n];
        loop {
            let mut changed = false;
            for i in 0..n {
                let mut acc: Option<BTreeSet<String>> = None;
                for &(p, kind) in &preds[i] {
                    let contrib = if kind == EdgeKind::Tick {
                        BTreeSet::new()
                    } else {
                        must[p].clone()
                    };
                    acc = Some(match acc {
                        None => contrib,
                        Some(a) => a.intersection(&contrib).cloned().collect(),
                    });
                }
                let mut out = acc.unwrap_or_default();
                out.extend(self.nodes[i].writes.iter().cloned());
                if out != must[i] {
                    must[i] = out;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // At the body's exit, an output written on some path but not all was
        // partially assigned — a latch. Report deterministically (sorted BTreeSet).
        for o in &self.comb_outputs {
            if may[exit].contains(o) && !must[exit].contains(o) {
                return Err((
                    self.nodes[exit].span,
                    format!(
                        "would infer a latch: combinational output `{o}` is assigned on some \
                         control paths but not all. Assign `{o}` on every path (add the missing \
                         branch / `_` arm)"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Backward-liveness fixpoint. Returns `live_out[i]` for every node. Edge kind is
    /// irrelevant to the propagation — a value used after a tick is still live
    /// *across* it, which is exactly what register inference keys on.
    fn liveness(&self) -> Vec<BTreeSet<String>> {
        let n = self.nodes.len();
        let mut live_in: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
        let mut live_out: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
        loop {
            let mut changed = false;
            // Reverse node order speeds convergence (successors tend to have higher
            // indices in this forward-built graph); correctness is order-independent.
            for i in (0..n).rev() {
                let mut out = BTreeSet::new();
                for &(s, _) in &self.nodes[i].succs {
                    out.extend(live_in[s].iter().cloned());
                }
                let mut in_ = self.nodes[i].uses.clone();
                for v in &out {
                    if !self.nodes[i].defs.contains(v) {
                        in_.insert(v.clone());
                    }
                }
                if out != live_out[i] {
                    live_out[i] = out;
                    changed = true;
                }
                if in_ != live_in[i] {
                    live_in[i] = in_;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        live_out
    }

    /// A minimal FSM report that falls out of the CFG for free (the item-2 byproduct
    /// tracked in `TODO`): the tick count (≈ number of cycle boundaries), the clock
    /// receivers seen, and the inferred registers. Kept intentionally small; a
    /// richer states/transitions dump can grow from the same `nodes` later.
    pub fn fsm_report(&self, module: &str) -> String {
        let ticks = self.nodes.iter().filter(|n| n.is_tick).count();
        let mut clocks: BTreeSet<&str> = BTreeSet::new();
        for n in &self.nodes {
            if let Some(c) = &n.tick_clock {
                clocks.insert(c);
            }
        }
        format!(
            "module {module}: {ticks} tick boundary(ies), clock(s) {:?}, registers {:?}",
            clocks.into_iter().collect::<Vec<_>>(),
            self.registers()
        )
    }
}

// ── CFG construction ────────────────────────────────────────────────────────

struct Builder {
    nodes: Vec<Node>,
    /// The module's combinational output ports, so terminal `port.write(…)` nodes
    /// can be tagged with the port they drive (definite-assignment).
    outputs: BTreeSet<String>,
    /// The module's `In` ports — propagated into nested-loop sub-CFGs so the
    /// multi-write-collapse detector can spot a leading read inside a nested loop.
    inputs: BTreeSet<String>,
    /// The locals bound to a `Memory<…>`, so a `read_port`/`write_port` call can be
    /// told from a same-named method on anything else. Propagated into nested-loop
    /// sub-CFGs for the same reason the port sets are.
    mems: BTreeSet<String>,
    /// Sub-CFGs of **tick-containing nested loops** — built alongside the (still
    /// folded) parent node so [`Cfg::check_reachability`] can recurse into each one
    /// and enforce the tickless-cycle invariant *inside* the nested loop. Only
    /// tick-containing loops are recorded; a tick-free nested loop is combinational
    /// (unrolled) and not subject to the "must reach a tick" rule.
    nested: Vec<Cfg>,
    /// Enclosing-loop targets while building a nested loop body: `(continue, break)`
    /// node indices. `continue`/fall-through routes to the loop head, `break` to the
    /// loop's exit sink — so a `break` before a tick does not read as a tickless
    /// cycle. Empty while building a top-level (hardware) loop, which never breaks.
    loop_ctx: Vec<(usize, usize)>,
    /// The top-level hardware loop's head, for a `continue` written directly in it.
    ///
    /// `loop_ctx` stays empty there because a hardware loop never `break`s, and a
    /// top-level `continue` was consequently routed to its FALL-THROUGH — modelling
    /// it as "carry on with the next statement", which is the one thing it does not
    /// do. `loop { if c { continue; } clk.tick().await; }` then reached the tick on
    /// every path and passed, while the real program returns to the head having
    /// ticked zero times: a zero-time cycle the simulator livelocks on. Same class
    /// as the nested-loop hole cause K uncovered, and it had to be closed before
    /// codegen could emit a `continue` at all.
    top_head: Option<usize>,
}

impl Builder {
    fn new(outputs: BTreeSet<String>, inputs: BTreeSet<String>, mems: BTreeSet<String>) -> Builder {
        Builder {
            nodes: Vec::new(),
            outputs,
            inputs,
            mems,
            nested: Vec::new(),
            loop_ctx: Vec::new(),
            top_head: None,
        }
    }
}

impl Builder {
    fn new_node(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Build a real sub-CFG of a nested loop's body: `head → body → head` with a
    /// dedicated exit sink as the `break` target. Fresh `Builder`, so it collects
    /// its own grandchild nested loops. Used only for reachability of the nested
    /// loop (no register/output analysis), so `defined_in_loop`/`comb_outputs`/
    /// `exit` are empty/`None`.
    fn nested_loop_cfg(&self, body: &[Stmt], span: Span) -> Cfg {
        let mut b = Builder::new(self.outputs.clone(), self.inputs.clone(), self.mems.clone());
        let head = b.new_node(Node::empty(span));
        let brk = b.new_node(Node::empty(span)); // exit sink for `break`
        b.loop_ctx.push((head, brk));
        let body_entry = b.build_block(body, head); // fall-through / `continue` → head
        b.loop_ctx.pop();
        b.nodes[head].succs.push((body_entry, EdgeKind::Comb));
        Cfg {
            nodes: b.nodes,
            head,
            defined_in_loop: BTreeSet::new(),
            // Port sets propagated from the parent so multi_write_collapse can recurse
            // into this nested loop (reachability, its original purpose, ignores them).
            comb_outputs: self.outputs.clone(),
            inputs: self.inputs.clone(),
            exit: None,
            nested_ticking_loops: b.nested,
        }
    }

    /// Build a straight-line block, threading `next` as the successor of the last
    /// statement. Processed in reverse so each statement's successor is already
    /// built when the statement is lowered; returns the entry node of the block.
    fn build_block(&mut self, stmts: &[Stmt], next: usize) -> usize {
        let mut cur = next;
        for stmt in stmts.iter().rev() {
            cur = self.build_stmt(stmt, cur);
        }
        cur
    }

    fn build_stmt(&mut self, stmt: &Stmt, next: usize) -> usize {
        match stmt {
            Stmt::Local(local) => {
                let mut defs = BTreeSet::new();
                pat_bindings(&local.pat, &mut defs);
                let mut uses = BTreeSet::new();
                let mut mem = Vec::new();
                if let Some(init) = &local.init {
                    collect_reads(&init.expr, &mut uses);
                    mem = mem_accesses(&init.expr, &self.mems);
                }
                self.terminal(defs, uses, mem, local.span(), next)
            }
            Stmt::Expr(expr, _) => self.build_expr(expr, next),
            Stmt::Macro(m) => {
                let mut uses = BTreeSet::new();
                collect_token_idents(&m.mac.tokens, &mut uses);
                self.terminal(BTreeSet::new(), uses, Vec::new(), m.span(), next)
            }
            Stmt::Item(item) => {
                self.terminal(BTreeSet::new(), BTreeSet::new(), Vec::new(), item.span(), next)
            }
        }
    }

    /// Lower a statement-position expression, expanding `if`/`match`/`tick`
    /// structurally so ticks nested in branches produce real tick edges.
    fn build_expr(&mut self, expr: &Expr, next: usize) -> usize {
        if let Some(clock) = tick_clock(expr) {
            return self.new_node(Node {
                uses: BTreeSet::from([clock.clone()]),
                is_tick: true,
                tick_clock: Some(clock),
                succs: vec![(next, EdgeKind::Tick)],
                ..Node::empty(expr.span())
            });
        }
        match expr {
            Expr::If(ei) => {
                let then_entry = self.build_block(&ei.then_branch.stmts, next);
                let else_entry = match &ei.else_branch {
                    Some((_, e)) => self.build_expr(e, next),
                    None => next,
                };
                let mut uses = BTreeSet::new();
                collect_reads(&ei.cond, &mut uses);
                self.new_node(Node {
                    uses,
                    mem: mem_accesses(&ei.cond, &self.mems),
                    succs: vec![(then_entry, EdgeKind::Comb), (else_entry, EdgeKind::Comb)],
                    ..Node::empty(ei.span())
                })
            }
            Expr::Match(em) => {
                let mut succs = Vec::with_capacity(em.arms.len());
                for arm in &em.arms {
                    let body_entry = self.build_expr(&arm.body, next);
                    let mut arm_defs = BTreeSet::new();
                    pat_bindings(&arm.pat, &mut arm_defs);
                    let mut arm_uses = BTreeSet::new();
                    let mut arm_mem = Vec::new();
                    if let Some((_, guard)) = &arm.guard {
                        collect_reads(guard, &mut arm_uses);
                        arm_mem = mem_accesses(guard, &self.mems);
                    }
                    // A pattern binding / guard needs its own entry node so its
                    // defs/uses sit on the path into the arm; otherwise route
                    // straight to the arm body.
                    let entry = if arm_defs.is_empty() && arm_uses.is_empty() && arm_mem.is_empty()
                    {
                        body_entry
                    } else {
                        self.new_node(Node {
                            defs: arm_defs,
                            uses: arm_uses,
                            mem: arm_mem,
                            succs: vec![(body_entry, EdgeKind::Comb)],
                            ..Node::empty(arm.pat.span())
                        })
                    };
                    succs.push((entry, EdgeKind::Comb));
                }
                let mut uses = BTreeSet::new();
                collect_reads(&em.expr, &mut uses);
                self.new_node(Node {
                    uses,
                    mem: mem_accesses(&em.expr, &self.mems),
                    succs,
                    ..Node::empty(em.span())
                })
            }
            // A bare block is just its statements.
            Expr::Block(b) => self.build_block(&b.block.stmts, next),
            // `break` / `continue` inside a nested loop body: route to that loop's
            // exit sink / head (from `loop_ctx`), not to the fall-through `next`, so
            // a `break` before a tick is not misread as a tickless cycle. At the top
            // level `loop_ctx` is empty (a hardware loop never breaks) → fall through.
            Expr::Break(b) => {
                let target = self.loop_ctx.last().map_or(next, |&(_, brk)| brk);
                self.new_node(Node {
                    succs: vec![(target, EdgeKind::Comb)],
                    ..Node::empty(b.span())
                })
            }
            Expr::Continue(c) => {
                let target = self
                    .loop_ctx
                    .last()
                    .map(|&(cont, _)| cont)
                    .or(self.top_head)
                    .unwrap_or(next);
                self.new_node(Node {
                    succs: vec![(target, EdgeKind::Comb)],
                    ..Node::empty(c.span())
                })
            }
            // A **nested** loop (`for` / `while` / `loop`). In the *parent* graph it
            // stays folded into a single node. If it *contains a tick* its out-edge is
            // a **Tick** edge (a clock boundary for the parent's liveness) and — new in
            // the nested-loop builder — its body is *also* built as a real sub-CFG
            // (`nested`) so the tickless-cycle invariant is enforced *inside* it
            // (recursively, with `break`/`continue` modeled). A tick-free nested loop
            // is combinational (unrolled) and neither ticks nor is checked. `defs`
            // stays empty (don't kill across the opaque region); interior reads → `uses`.
            //
            // The Tick out-edge used to make the fold *unconditionally* optimistic —
            // "a possible 0-iteration exit must not make the outer loop look tickless",
            // so that a design ticking only inside a `for` (`uart_tx`, `rv32i_cpu`)
            // stays well-formed. That is right for a counted `for`, which runs its body
            // and therefore ticks; it is **wrong** for a `loop`/`while` that can break
            // before its first tick, where the zero-tick exit is a real path. An
            // enclosing loop whose only boundary is such a nested loop then cycles in
            // zero time — measured: the simulator livelocks (99.5% CPU, no progress)
            // while the flattened FSM runs one cycle per iteration. `may_exit_without_tick`
            // records that path for `check_reachability` WITHOUT weakening the Tick
            // edge, which liveness still needs (see the field's docs).
            Expr::While(_) | Expr::ForLoop(_) | Expr::Loop(_) => {
                let mut uses = BTreeSet::new();
                collect_reads(expr, &mut uses);
                let clock = first_tick_clock(expr);
                let (is_tick, kind) = match &clock {
                    Some(_) => (true, EdgeKind::Tick),
                    None => (false, EdgeKind::Comb),
                };
                // Memory accesses inside the loop. For a TICK-FREE loop the whole
                // body runs in this cycle (and is refused outright by the staging
                // check, since unrolling puts every iteration's access on one bus).
                // For a tick-bearing one only the accesses before its FIRST tick run
                // in the cycle that reaches this node; the rest belong to later
                // cycles and are checked in the sub-CFG, where those ticks are real
                // edges. Attributing all of them here read `rv32i_cpu`'s
                // `read(addr); loop { tick; if is_ready { break } }` — the standard
                // wait — as a same-cycle observation.
                let mut mem = mem_accesses(expr, &self.mems);
                if is_tick {
                    let sub = self.nested_loop_cfg(loop_body_stmts(expr), expr.span());
                    mem = sub.entry_cycle_mem();
                    self.nested.push(sub);
                }
                self.new_node(Node {
                    uses,
                    is_tick,
                    tick_clock: clock,
                    mem,
                    may_exit_without_tick: is_tick && may_exit_without_tick(expr),
                    succs: vec![(next, kind)],
                    // Opaque region: conservatively assume it drives every output
                    // (so the definite-assignment check never *false*-flags a partial
                    // write around a folded loop — it under-reports here, by design).
                    writes: self.outputs.clone(),
                    folded: true,
                    ..Node::empty(expr.span())
                })
            }
            // Assignment / compound-assign / method call / `port.write(..)` / other
            // terminal expression: one node, combinational edge to the continuation.
            _ => {
                let (defs, uses) = terminal_defs_uses(expr);
                let writes = output_write_target(expr, &self.outputs)
                    .into_iter()
                    .collect();
                self.new_node(Node {
                    defs,
                    uses,
                    mem: mem_accesses(expr, &self.mems),
                    succs: vec![(next, EdgeKind::Comb)],
                    writes,
                    ..Node::empty(expr.span())
                })
            }
        }
    }

    fn terminal(
        &mut self,
        defs: BTreeSet<String>,
        uses: BTreeSet<String>,
        mem: Vec<MemAccess>,
        span: Span,
        next: usize,
    ) -> usize {
        self.new_node(Node {
            defs,
            uses,
            mem,
            succs: vec![(next, EdgeKind::Comb)],
            ..Node::empty(span)
        })
    }
}

// ── def / use extraction ────────────────────────────────────────────────────

/// Every memory-port access site in `expr` (recursively), for the memories in
/// `mems`. Restricting to *known* memory locals is what keeps an unrelated
/// `something.read_port::<0>()` out of the analysis; the set comes from
/// [`memory_locals`].
fn mem_accesses(expr: &Expr, mems: &BTreeSet<String>) -> Vec<MemAccess> {
    struct V<'a> {
        mems: &'a BTreeSet<String>,
        out: Vec<MemAccess>,
    }
    impl<'ast> Visit<'ast> for V<'_> {
        fn visit_expr_method_call(&mut self, mc: &'ast syn::ExprMethodCall) {
            if let Some((mem, port, is_read)) = mem_port_receiver(&mc.receiver, self.mems) {
                let kind = match (is_read, mc.method.to_string().as_str()) {
                    (true, "read") => Some(MemAccessKind::StageRead),
                    (false, "write") => Some(MemAccessKind::StageWrite),
                    (true, "data") => Some(MemAccessKind::Observe("data")),
                    (true, "is_ready") => Some(MemAccessKind::Observe("is_ready")),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.out.push(MemAccess { mem, port, kind });
                }
            }
            syn::visit::visit_expr_method_call(self, mc);
        }
    }
    if mems.is_empty() {
        return Vec::new();
    }
    let mut v = V { mems, out: Vec::new() };
    v.visit_expr(expr);
    v.out
}

/// If `recv` is `<mem>.read_port::<K>()` / `<mem>.write_port::<K>()` for a known
/// memory, return `(mem, K-as-text, is_read)`.
fn mem_port_receiver(recv: &Expr, mems: &BTreeSet<String>) -> Option<(String, String, bool)> {
    let Expr::MethodCall(mc) = recv else { return None };
    let is_read = match mc.method.to_string().as_str() {
        "read_port" => true,
        "write_port" => false,
        _ => return None,
    };
    if !mc.args.is_empty() {
        return None;
    }
    let mem = simple_ident(&mc.receiver)?;
    if !mems.contains(&mem) {
        return None;
    }
    let arg = mc.turbofish.as_ref()?.args.first()?;
    let port = match arg {
        syn::GenericArgument::Const(Expr::Lit(l)) => match &l.lit {
            syn::Lit::Int(i) => i.base10_digits().to_string(),
            _ => return None,
        },
        syn::GenericArgument::Const(e) => simple_ident(e)?,
        syn::GenericArgument::Type(syn::Type::Path(tp)) => {
            tp.path.get_ident().map(|i| i.to_string())?
        }
        _ => return None,
    };
    Some((mem, port, is_read))
}

/// The locals bound to a `Memory<…>` in `f` — `let mem = Memory::<…>::new(…)`,
/// including the `.write_first()` / `.read_first()` builder spellings. Declared
/// before the hardware loop, so the whole body is scanned.
fn memory_locals(f: &ItemFn) -> BTreeSet<String> {
    struct V {
        out: BTreeSet<String>,
    }
    impl<'ast> Visit<'ast> for V {
        fn visit_local(&mut self, l: &'ast syn::Local) {
            if let Some(init) = &l.init {
                if mentions_memory_type(&init.expr) {
                    pat_bindings(&l.pat, &mut self.out);
                }
            }
            syn::visit::visit_local(self, l);
        }
    }
    let mut v = V { out: BTreeSet::new() };
    v.visit_block(&f.block);
    v.out
}

/// Does `expr` name the `Memory` type anywhere in a path (`Memory::<…>::new`)?
fn mentions_memory_type(expr: &Expr) -> bool {
    struct V {
        found: bool,
    }
    impl<'ast> Visit<'ast> for V {
        fn visit_path(&mut self, p: &'ast syn::Path) {
            if p.segments.iter().any(|s| s.ident == "Memory") {
                self.found = true;
            }
            syn::visit::visit_path(self, p);
        }
    }
    let mut v = V { found: false };
    v.visit_expr(expr);
    v.found
}

/// The `(defs, uses)` of a terminal (non-branch, non-tick) statement expression.
fn terminal_defs_uses(expr: &Expr) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut defs = BTreeSet::new();
    let mut uses = BTreeSet::new();
    match expr {
        Expr::Assign(a) => {
            assign_lhs(&a.left, &mut defs, &mut uses);
            collect_reads(&a.right, &mut uses);
        }
        Expr::Binary(b) if is_assign_op(&b.op) => {
            // Compound assign `x += y`: reads and writes `x`.
            if let Some(id) = simple_ident(&b.left) {
                defs.insert(id.clone());
                uses.insert(id);
            } else {
                collect_reads(&b.left, &mut uses);
            }
            collect_reads(&b.right, &mut uses);
        }
        _ => collect_reads(expr, &mut uses),
    }
    (defs, uses)
}

/// Split an assignment LHS into killing defs vs read-modify-write uses. A bare
/// `x` (or a tuple of them) is a full def; `x[i]`/`x.f` is a partial write, so `x`
/// stays a *use* (it isn't killed) — but see [`DefinedInLoop`], which still counts
/// `x` as assigned-in-loop so a partially-written register is a register candidate.
fn assign_lhs(left: &Expr, defs: &mut BTreeSet<String>, uses: &mut BTreeSet<String>) {
    match left {
        Expr::Path(_) => {
            if let Some(id) = simple_ident(left) {
                defs.insert(id);
            }
        }
        Expr::Tuple(t) => {
            for elem in &t.elems {
                assign_lhs(elem, defs, uses);
            }
        }
        Expr::Index(i) => {
            collect_reads(&i.expr, uses);
            collect_reads(&i.index, uses);
        }
        Expr::Field(f) => collect_reads(&f.base, uses),
        other => collect_reads(other, uses),
    }
}

/// Every variable **read** in `expr` (single-segment path identifiers). Method
/// names, multi-segment paths (enum variants like `State::A`), and struct-literal
/// field names are not idents-as-reads and are skipped; macro token streams are
/// scanned for idents (over-approx, but only affects vars that are also
/// assigned-in-loop, which is rare enough to be harmless in v1).
fn collect_reads(expr: &Expr, out: &mut BTreeSet<String>) {
    struct Reads<'a>(&'a mut BTreeSet<String>);
    impl<'ast> Visit<'ast> for Reads<'_> {
        fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
            if p.qself.is_none() && p.path.segments.len() == 1 {
                self.0.insert(p.path.segments[0].ident.to_string());
            }
        }
        fn visit_macro(&mut self, m: &'ast syn::Macro) {
            collect_token_idents(&m.tokens, self.0);
        }
    }
    Reads(out).visit_expr(expr);
}

/// Insert every identifier token appearing in a macro's token stream.
fn collect_token_idents(tokens: &TokenStream, out: &mut BTreeSet<String>) {
    for tt in tokens.clone() {
        match tt {
            TokenTree::Ident(id) => {
                out.insert(id.to_string());
            }
            TokenTree::Group(g) => collect_token_idents(&g.stream(), out),
            _ => {}
        }
    }
}

/// Collect the identifiers bound by a pattern (recursively).
fn pat_bindings(pat: &Pat, out: &mut BTreeSet<String>) {
    match pat {
        Pat::Ident(pi) => {
            out.insert(pi.ident.to_string());
            if let Some((_, sub)) = &pi.subpat {
                pat_bindings(sub, out);
            }
        }
        Pat::Type(pt) => pat_bindings(&pt.pat, out),
        Pat::Reference(r) => pat_bindings(&r.pat, out),
        Pat::Paren(p) => pat_bindings(&p.pat, out),
        Pat::Tuple(t) => t.elems.iter().for_each(|e| pat_bindings(e, out)),
        Pat::TupleStruct(ts) => ts.elems.iter().for_each(|e| pat_bindings(e, out)),
        Pat::Slice(s) => s.elems.iter().for_each(|e| pat_bindings(e, out)),
        Pat::Or(o) => o.cases.iter().for_each(|c| pat_bindings(c, out)),
        Pat::Struct(s) => s.fields.iter().for_each(|f| pat_bindings(&f.pat, out)),
        _ => {}
    }
}

/// Visitor collecting the register **candidates**: every variable with a def site
/// (let-binding or assignment target, including partial-write and tuple targets)
/// anywhere inside the loop body. Deliberately *not* collecting match-arm pattern
/// bindings — those are transient arm-local names, not persistent state.
struct DefinedInLoop<'a> {
    set: &'a mut BTreeSet<String>,
}

impl<'ast> Visit<'ast> for DefinedInLoop<'_> {
    fn visit_local(&mut self, l: &'ast syn::Local) {
        pat_bindings(&l.pat, self.set);
        syn::visit::visit_local(self, l);
    }
    /// A **ticking** `for`'s binding is defined in the loop just as a `let` is, and
    /// it is read on the far side of the tick its own body contains — so it is a
    /// register, and the transpiler builds one for it (control extraction desugars
    /// the counted `for` into a counter-driven `loop`). Without this the shared
    /// inference reported no register for `for i in 0..8 { …; tick; }` while
    /// codegen emitted `i`, and the two front-ends disagreed about the language's
    /// central rule.
    ///
    /// A tick-FREE `for` is combinational and unrolls, so its variable is an
    /// elaboration-time index and not state. `_` binds nothing and contributes
    /// nothing, which is correct: the counter the transpiler synthesizes for it has
    /// no source-level name, exactly like `pc`.
    fn visit_expr_for_loop(&mut self, f: &'ast syn::ExprForLoop) {
        if f.body.stmts.iter().any(stmt_contains_tick) {
            pat_bindings(&f.pat, self.set);
        }
        syn::visit::visit_expr_for_loop(self, f);
    }
    fn visit_expr_assign(&mut self, a: &'ast syn::ExprAssign) {
        assign_targets(&a.left, self.set);
        syn::visit::visit_expr_assign(self, a);
    }
    fn visit_expr_binary(&mut self, b: &'ast syn::ExprBinary) {
        if is_assign_op(&b.op) {
            assign_targets(&b.left, self.set);
        }
        syn::visit::visit_expr_binary(self, b);
    }
}

/// The base variable names written by an assignment LHS — bare, tuple, indexed, or
/// field — so a partially-written register still counts as assigned-in-loop.
fn assign_targets(left: &Expr, out: &mut BTreeSet<String>) {
    match left {
        Expr::Path(_) => {
            if let Some(id) = simple_ident(left) {
                out.insert(id);
            }
        }
        Expr::Tuple(t) => t.elems.iter().for_each(|e| assign_targets(e, out)),
        Expr::Index(i) => assign_targets(&i.expr, out),
        Expr::Field(f) => assign_targets(&f.base, out),
        _ => {}
    }
}

// ── small syntactic helpers ─────────────────────────────────────────────────

/// The module's **combinational** output ports: signature parameters whose type
/// is `Out<…>` (exactly — `RegOut` is a registered output and is excluded, as are
/// `In`/`Clock`/`Memory`). These are the ports the definite-assignment check
/// requires to be driven on all-or-no paths per cycle.
fn combinational_outputs(f: &ItemFn) -> BTreeSet<String> {
    let mut outs = BTreeSet::new();
    for arg in &f.sig.inputs {
        if let syn::FnArg::Typed(pt) = arg {
            if matches!(&*pt.ty, syn::Type::Path(tp)
                if tp.path.segments.last().is_some_and(|s| s.ident == "Out"))
            {
                pat_bindings(&pt.pat, &mut outs);
            }
        }
    }
    outs
}

/// `Some(port)` iff `expr` is `<port>.write(…)` for a `port` in `outputs`.
fn output_write_target(expr: &Expr, outputs: &BTreeSet<String>) -> Option<String> {
    let Expr::MethodCall(mc) = expr else { return None };
    if mc.method != "write" {
        return None;
    }
    simple_ident(&mc.receiver).filter(|r| outputs.contains(r))
}

/// The statement list and span of the module's top-level `loop { … }`, if any.
fn top_level_loop(f: &ItemFn) -> Option<(Vec<Stmt>, Span)> {
    f.block.stmts.iter().find_map(|s| match s {
        Stmt::Expr(Expr::Loop(l), _) => Some((l.body.stmts.clone(), l.span())),
        _ => None,
    })
}

/// The statement list of a nested `while` / `for` / `loop` body.
fn loop_body_stmts(expr: &Expr) -> &[Stmt] {
    match expr {
        Expr::While(w) => &w.body.stmts,
        Expr::ForLoop(f) => &f.body.stmts,
        Expr::Loop(l) => &l.body.stmts,
        _ => &[],
    }
}

/// Can this **tick-bearing** nested loop be left without ever reaching a tick?
///
/// Only the *zero-tick exit* is asked about here; a loop that can cycle internally
/// without ticking is a different (and separately enforced) defect, caught by
/// `check_reachability` recursing into the loop's own sub-CFG.
///
/// * `loop` — exits only by `break`, so the answer is "yes" unless every top-level
///   path reaches a tick before any `break` can be taken.
/// * `while` — the test can be false on entry, so it always can (a zero-iteration
///   exit needs no `break` at all). This matches what the transpiler sees: a
///   tick-bearing `while` is desugared to `loop { if !cond { break; } … }`, whose
///   leading `break` gives the same answer. The two front-ends agreeing about this
///   is the point — them disagreeing is this pipeline's recurring bug class.
/// * `for` — assumed to run its body, so "no". A counted repetition over a constant
///   range is the shape the fold's optimism was built for (`uart_tx`, `rv32i_cpu`,
///   `for _ in 0..CLKS_PER_BIT { clk.tick().await; }`) and it genuinely does tick.
///   The assumption is only wrong for an **empty** range, which needs const
///   evaluation to see and which this crate (syntax-level, `syn`-only) cannot do.
///   Recorded rather than silently relied on: an empty constant range would make
///   the enclosing loop tickless and is not detected here.
fn may_exit_without_tick(expr: &Expr) -> bool {
    match expr {
        Expr::ForLoop(_) => false,
        Expr::While(_) => true,
        Expr::Loop(l) => !ticks_before_any_break(&l.body.stmts),
        _ => false,
    }
}

/// Does every top-level path through this loop body reach a tick before it can
/// `break` out of the loop?
///
/// Deliberately conservative: a tick that sits inside a branch (`if c { tick; }`)
/// answers "no", because the other branch reaches the body's end — and thus the
/// loop head — without ticking. Only a tick in *statement* position counts, or a
/// nested loop that is itself a guaranteed boundary (mutually recursive with
/// [`may_exit_without_tick`]; both descend structurally, so this terminates).
fn ticks_before_any_break(body: &[Stmt]) -> bool {
    for stmt in body {
        let Stmt::Expr(e, _) = stmt else {
            // A `let` whose initializer breaks or ticks is not a shape this walk
            // models; treat it as neither, and keep looking.
            continue;
        };
        if tick_clock(e).is_some() {
            return true; // an unconditional clock boundary, reached first
        }
        if matches!(e, Expr::Loop(_) | Expr::While(_) | Expr::ForLoop(_)) {
            // A nested loop that always ticks is itself a boundary: reaching it
            // guarantees a clock edge. Without this clause `loop { <work>; for _ in
            // 0..N { tick; } }` — the shape cause K exists to support — would read
            // as tickless and be false-rejected.
            if first_tick_clock(e).is_some() && !may_exit_without_tick(e) {
                return true;
            }
            return false;
        }
        if breaks_enclosing_loop(e) {
            return false; // an exit is reachable with no tick behind it
        }
        if first_tick_clock(e).is_some() {
            return false; // ticks only on *some* path through this statement
        }
    }
    false // fell out of the body without ticking
}

/// Does `expr` contain a `break` belonging to the loop that encloses it? A `break`
/// under a nested loop of its own belongs to *that* loop and does not count.
fn breaks_enclosing_loop(expr: &Expr) -> bool {
    match expr {
        Expr::Break(_) => true,
        Expr::If(f) => {
            f.then_branch.stmts.iter().any(stmt_breaks_enclosing_loop)
                || f.else_branch.as_ref().is_some_and(|(_, e)| breaks_enclosing_loop(e))
        }
        Expr::Block(b) => b.block.stmts.iter().any(stmt_breaks_enclosing_loop),
        Expr::Match(m) => m.arms.iter().any(|a| breaks_enclosing_loop(&a.body)),
        Expr::Loop(_) | Expr::While(_) | Expr::ForLoop(_) => false,
        _ => false,
    }
}

fn stmt_breaks_enclosing_loop(s: &Stmt) -> bool {
    match s {
        Stmt::Expr(e, _) => breaks_enclosing_loop(e),
        _ => false,
    }
}

/// `Some(clock_name)` iff `expr` is `<clock>.tick().await`. The clock receiver
/// identity is preserved (item 4's per-domain tick tag); a non-simple receiver
/// yields the placeholder `"<clock>"`.
fn tick_clock(expr: &Expr) -> Option<String> {
    let Expr::Await(a) = expr else { return None };
    let Expr::MethodCall(mc) = a.base.as_ref() else {
        return None;
    };
    if mc.method != "tick" || !mc.args.is_empty() {
        return None;
    }
    Some(simple_ident(&mc.receiver).unwrap_or_else(|| "<clock>".to_string()))
}

/// Does this statement issue a `<clock>.tick().await` at any depth? Shares
/// [`first_tick_clock`]'s walk so the two cannot disagree about where a tick can
/// live — a drift that has caused real bugs in the transpiler's own gate.
fn stmt_contains_tick(s: &syn::Stmt) -> bool {
    struct FindTick(bool);
    impl<'ast> Visit<'ast> for FindTick {
        fn visit_expr(&mut self, e: &'ast Expr) {
            if self.0 {
                return;
            }
            if tick_clock(e).is_some() {
                self.0 = true;
                return;
            }
            syn::visit::visit_expr(self, e);
        }
    }
    let mut f = FindTick(false);
    f.visit_stmt(s);
    f.0
}

/// The clock receiver of the first `<clock>.tick().await` anywhere inside `expr`,
/// or `None` if `expr` contains no tick. Used to decide whether a folded nested
/// loop crosses a clock edge (and, for item 4, which domain it belongs to).
fn first_tick_clock(expr: &Expr) -> Option<String> {
    struct FindTick(Option<String>);
    impl<'ast> Visit<'ast> for FindTick {
        fn visit_expr(&mut self, e: &'ast Expr) {
            if self.0.is_some() {
                return;
            }
            if let Some(c) = tick_clock(e) {
                self.0 = Some(c);
                return;
            }
            syn::visit::visit_expr(self, e);
        }
    }
    let mut f = FindTick(None);
    f.visit_expr(expr);
    f.0
}

/// The single identifier of a bare path expression (`x`), else `None`.
fn simple_ident(e: &Expr) -> Option<String> {
    if let Expr::Path(p) = e {
        if p.qself.is_none() && p.path.segments.len() == 1 {
            return Some(p.path.segments[0].ident.to_string());
        }
    }
    None
}

fn is_assign_op(op: &BinOp) -> bool {
    matches!(
        op,
        BinOp::AddAssign(_)
            | BinOp::SubAssign(_)
            | BinOp::MulAssign(_)
            | BinOp::DivAssign(_)
            | BinOp::RemAssign(_)
            | BinOp::BitXorAssign(_)
            | BinOp::BitAndAssign(_)
            | BinOp::BitOrAssign(_)
            | BinOp::ShlAssign(_)
            | BinOp::ShrAssign(_)
    )
}

// ── read-timing classification (item 3) ─────────────────────────────────────
//
// The compile-time replacement for the runtime read-freshness oracle
// (`copper-sim/src/synced_read.rs`). Each `In`-parameter `.read()` site is
// classified statically as [`ReadTiming::Deferred`] or [`ReadTiming::Immediate`]
// by its position relative to clock ticks in the loop iteration; the macro bakes
// that classification into the generated sim code (a deferred read gets a
// `pre_edge_barrier().await` before it; an immediate read is a plain `.read()`),
// so at runtime there is no timing heuristic — just the accepted phase machinery.
//
// **The rule.** A read is `Deferred` iff a clock tick occurs *after* it within
// the loop iteration's control flow (some continuation path from the read reaches
// a `clk.tick().await` before the iteration closes) — a "leading"/pre-tick read
// that registers its input at that edge, so it samples at the next pre-edge
// settle. Otherwise it is `Immediate` — a trailing/post-tick read that consumes
// the value the just-past edge produced and fires without deferral.
//
// This reproduces the timing the current heuristic gets right (loop-top reads in
// `mac_pipeline`/`sipo_block` defer; the trailing next-state reads in `counter`/
// `traffic_light` fire immediately) and fixes the class it gets wrong (the
// variable-iteration `while in_i.read() == 0 { tick }` reads in `det_010_awaits`,
// which a runtime phase/call-id heuristic phase-shifts by path history — see the
// impl plan's item 3).

/// The compile-time edge-phase classification of one `In`-parameter `.read()`
/// site. See the module note above for the rule and rationale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReadTiming {
    /// A "leading"/pre-tick read: a clock tick follows it within the iteration, so
    /// its result is registered at that edge. Sampled at the next pre-edge settle
    /// (deferred on loop re-entry). Generated code: `pre_edge_barrier().await`
    /// before the `.read()`.
    Deferred,
    /// A trailing/post-tick read: no tick follows it before the iteration closes,
    /// so it consumes the value the just-past edge produced. Sampled immediately.
    /// Generated code: a plain `.read()`.
    Immediate,
}

/// Classify every `In`-parameter `.read()` site of `f`, in source (left-to-right,
/// pre-order) order — the identical order `copper-macros`'s read rewriter visits
/// them, so the two correlate by index without relying on spans (which do not
/// survive the transpiler's re-parse). Returns an empty vector for a function with
/// no `In` parameters (a free-running module has nothing to classify).
///
/// The i-th entry is the timing of the i-th `In`-param read; the macro assigns the
/// i-th tag to the i-th read it rewrites.
pub fn classify_reads(f: &ItemFn) -> Vec<ReadTiming> {
    let in_params = in_param_names(f);
    if in_params.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();

    // A segment that assigns no register has nothing for a `pre_edge_barrier` to
    // pin, which is what licenses the passthrough case in `classify_expr`.
    // Registers are computed on the same `ItemFn`, so the two agree by construction.
    let comb_outputs = combinational_outputs(f);
    let combinational_segment = Cfg::build(f).is_some_and(|c| c.registers().is_empty());
    let ctx = ReadCtx {
        in_params: &in_params,
        comb_outputs: &comb_outputs,
        combinational_segment,
    };

    // The whole body is walked (reads before the top-level loop are sampled once
    // at spawn, where Deferred and Immediate coincide — both land at the initial
    // pre-edge). `tail_has_tick = false`: after the last statement of a loop
    // iteration control returns to the head, and the next iteration's tick is not
    // "after" this read within *this* iteration — which is exactly why a trailing
    // next-state read (`counter`, `traffic_light`) is Immediate, not Deferred.
    classify_block(&f.block.stmts, false, &ctx, &mut out);
    out
}

/// The names of the function's `In<T, D>` parameters (outer type `In`). Mirrors
/// the set `copper-macros` rewrites, so classification and rewrite cover the
/// identical read sites.
fn in_param_names(f: &ItemFn) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for arg in &f.sig.inputs {
        if let syn::FnArg::Typed(pt) = arg {
            let is_in = matches!(&*pt.ty, syn::Type::Path(tp)
                if tp.path.segments.last().is_some_and(|s| s.ident == "In"));
            if is_in {
                pat_bindings(&pt.pat, &mut names);
            }
        }
    }
    names
}

/// Shared inputs for read classification.
struct ReadCtx<'a> {
    in_params: &'a BTreeSet<String>,
    /// Plain combinational `Out` ports (not `RegOut`).
    comb_outputs: &'a BTreeSet<String>,
    /// True when the module's pre-tick segment assigns **no register**.
    ///
    /// The `pre_edge_barrier` a `Deferred` read injects does two jobs: it defers the
    /// read *and* it pins the whole segment to the pre-edge phase. Pinning is
    /// essential when the segment updates a register — that is the pre-tick alignment
    /// hazard. When the segment assigns nothing, there is nothing to pin, and
    /// deferring a read that only feeds a wire makes that wire behave like a flop.
    /// See [`passthrough_read`].
    combinational_segment: bool,
}

/// Whether `expr` is `<comb_out>.write(…)` — the shape whose arguments feed a wire
/// rather than a flop.
///
/// A read in this position, in a segment that assigns no register, is `Immediate`:
/// the value is consumed within the cycle by a continuous assignment
/// (`assign out = inp`), so deferring it makes the simulator trail its own netlist by
/// a cycle. Adjudicated against independent hand-written Verilog — a clocked producer
/// feeding a passthrough gives `mid == out` in hardware, and only the `Immediate`
/// form reproduces that.
///
/// Deliberately narrow. It does **not** apply to a read in a *condition* (those are
/// handled by the `If`/`Match`/`While` arms, which already defer a read gating a tick:
/// `det_010_awaits` and `if_tick` read inside control flow whose tick *count* depends
/// on the sampled value, so their phase genuinely matters), nor to any segment that
/// assigns a register.
fn passthrough_read(expr: &Expr, ctx: &ReadCtx<'_>) -> bool {
    if !ctx.combinational_segment {
        return false;
    }
    let Expr::MethodCall(mc) = expr else { return false };
    mc.method == "write"
        && simple_ident(&mc.receiver).is_some_and(|n| ctx.comb_outputs.contains(&n))
}

/// Classify the reads of a straight-line block. `tail_has_tick` is whether a tick
/// occurs *after* this block completes, in the enclosing continuation. Statements
/// are processed in source order (so emitted read indices match the macro), and
/// for each statement `after` is whether any tick follows it — either later in
/// this block or in the tail.
fn classify_block(
    stmts: &[Stmt],
    tail_has_tick: bool,
    ctx: &ReadCtx<'_>,
    out: &mut Vec<ReadTiming>,
) {
    for (i, stmt) in stmts.iter().enumerate() {
        let after = tail_has_tick || stmts[i + 1..].iter().any(stmt_has_tick);
        classify_stmt(stmt, after, ctx, out);
    }
}

fn classify_stmt(
    stmt: &Stmt,
    after: bool,
    ctx: &ReadCtx<'_>,
    out: &mut Vec<ReadTiming>,
) {
    match stmt {
        Stmt::Local(l) => {
            if let Some(init) = &l.init {
                classify_expr(&init.expr, after, ctx, out);
                if let Some((_, diverge)) = &init.diverge {
                    classify_expr(diverge, after, ctx, out);
                }
            }
        }
        Stmt::Expr(e, _) => classify_expr(e, after, ctx, out),
        // Reads inside a macro token stream are *not* rewritten by the macro
        // (`syn` does not descend into macro tokens), so they are not classified
        // either — keeping the two sides' read sets identical.
        Stmt::Macro(_) | Stmt::Item(_) => {}
    }
}

/// Classify the reads of an expression. `after` = whether a tick follows this
/// whole expression in the iteration. Control-flow constructs (`if`/`match`/loops)
/// are handled explicitly so a read in a condition/scrutinee sees a tick that
/// lives in a *branch* (that is what makes `det_010_awaits`'s nested-`if` reads
/// Deferred). Every other expression is a value expression with no interior tick
/// (ticks are statements in hardware bodies), so all its reads share `after`.
fn classify_expr(
    expr: &Expr,
    after: bool,
    ctx: &ReadCtx<'_>,
    out: &mut Vec<ReadTiming>,
) {
    // A `clk.tick().await` contributes no `In`-param read.
    if tick_clock(expr).is_some() {
        return;
    }
    // `<comb_out>.write(<expr>)` in a register-free segment: the arguments feed a
    // continuous assignment, so their reads are Immediate regardless of a following
    // tick. See `passthrough_read`.
    if passthrough_read(expr, ctx) {
        if let Expr::MethodCall(mc) = expr {
            for arg in &mc.args {
                classify_expr(arg, false, ctx, out);
            }
            return;
        }
    }
    match expr {
        // `<in_param>.read()` — the classified site.
        Expr::MethodCall(mc) if mc.method == "read" && mc.args.is_empty() => {
            if let Some(name) = simple_ident(&mc.receiver) {
                if ctx.in_params.contains(&name) {
                    out.push(if after {
                        ReadTiming::Deferred
                    } else {
                        ReadTiming::Immediate
                    });
                    return;
                }
            }
            // A `.read()` on something that is not a bare `In` param (e.g. a method
            // chain): still descend so any nested `In` reads are covered.
            classify_expr(&mc.receiver, after, ctx, out);
        }
        Expr::If(ei) => {
            let then_tick = block_has_tick(&ei.then_branch.stmts);
            let else_tick = ei
                .else_branch
                .as_ref()
                .is_some_and(|(_, e)| expr_has_tick(e));
            // A condition read is Deferred if a tick follows the whole `if` OR a
            // tick lives in either branch (the read gates that tick's edge).
            classify_expr(&ei.cond, after || then_tick || else_tick, ctx, out);
            classify_block(&ei.then_branch.stmts, after, ctx, out);
            if let Some((_, e)) = &ei.else_branch {
                classify_expr(e, after, ctx, out);
            }
        }
        Expr::Match(em) => {
            let arm_tick = em.arms.iter().any(|a| {
                expr_has_tick(&a.body)
                    || a.guard.as_ref().is_some_and(|(_, g)| expr_has_tick(g))
            });
            classify_expr(&em.expr, after || arm_tick, ctx, out);
            for arm in &em.arms {
                if let Some((_, g)) = &arm.guard {
                    classify_expr(g, after, ctx, out);
                }
                classify_expr(&arm.body, after, ctx, out);
            }
        }
        // A `while cond { body }`: a condition read is Deferred if the body ticks
        // (the read gates the body's edge) or a tick follows the loop. The body is
        // a fresh iteration (`tail_has_tick = false`), like the top-level loop.
        Expr::While(w) => {
            classify_expr(
                &w.cond,
                after || block_has_tick(&w.body.stmts),
                ctx,
                out,
            );
            classify_block(&w.body.stmts, false, ctx, out);
        }
        Expr::ForLoop(fl) => {
            classify_expr(&fl.expr, after, ctx, out);
            classify_block(&fl.body.stmts, false, ctx, out);
        }
        Expr::Loop(l) => classify_block(&l.body.stmts, false, ctx, out),
        Expr::Block(b) => classify_block(&b.block.stmts, after, ctx, out),
        // Any other expression is a value expression: no interior tick, so all its
        // `In` reads share `after`. Flat-collect them in source order (the same
        // order the macro's `visit_expr_mut` reaches the read leaves).
        _ => {
            let timing = if after {
                ReadTiming::Deferred
            } else {
                ReadTiming::Immediate
            };
            collect_reads_flat(expr, timing, ctx.in_params, out)
        }
    }
}

/// Emit `timing` for every `In`-param `.read()` reachable in `expr`, in the source
/// order `syn`'s visitor yields (which matches the macro's rewrite order). Used for
/// value expressions, where there is no interior tick to change the timing.
fn collect_reads_flat(
    expr: &Expr,
    timing: ReadTiming,
    in_params: &BTreeSet<String>,
    out: &mut Vec<ReadTiming>,
) {
    struct Flat<'a> {
        timing: ReadTiming,
        in_params: &'a BTreeSet<String>,
        out: &'a mut Vec<ReadTiming>,
    }
    impl<'ast> Visit<'ast> for Flat<'_> {
        fn visit_expr_method_call(&mut self, mc: &'ast syn::ExprMethodCall) {
            if mc.method == "read" && mc.args.is_empty() {
                if let Some(name) = simple_ident(&mc.receiver) {
                    if self.in_params.contains(&name) {
                        self.out.push(self.timing);
                        return;
                    }
                }
            }
            syn::visit::visit_expr_method_call(self, mc);
        }
    }
    Flat {
        timing,
        in_params,
        out,
    }
    .visit_expr(expr);
}

/// Whether a statement contains a `clk.tick().await` anywhere within it.
fn stmt_has_tick(stmt: &Stmt) -> bool {
    struct FindTick(bool);
    impl<'ast> Visit<'ast> for FindTick {
        fn visit_expr(&mut self, e: &'ast Expr) {
            if self.0 {
                return;
            }
            if tick_clock(e).is_some() {
                self.0 = true;
                return;
            }
            syn::visit::visit_expr(self, e);
        }
    }
    let mut f = FindTick(false);
    f.visit_stmt(stmt);
    f.0
}

/// Whether an expression contains a tick anywhere within it.
fn expr_has_tick(expr: &Expr) -> bool {
    first_tick_clock(expr).is_some()
}

/// Whether a block's statements contain a tick.
fn block_has_tick(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_has_tick)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> ItemFn {
        syn::parse_str(src).expect("parse ItemFn")
    }

    fn regs(src: &str) -> Vec<String> {
        Cfg::build(&parse(src)).map(|c| c.registers()).unwrap_or_default()
    }

    // ── register inference (backward liveness) ──────────────────────────────

    /// `mac_fsm`: four pre-loop locals, all reassigned in the loop and read a
    /// cycle later — the G6-slice shape, still correct under general liveness.
    #[test]
    fn mac_fsm_registers() {
        let src = r#"
            #[hardware(sequential)]
            async fn mac_fsm(clk: Clock<C>, a: In<Bits<8>, C>, b: In<Bits<8>, C>, c: In<Bits<8>, C>, out: RegOut<Bits<8>, C>) {
                let mut stage = Stage::Load;
                let mut product: Bits<8> = Bits::from_lit::<0>();
                let mut c_latch: Bits<8> = Bits::from_lit::<0>();
                let mut result: Bits<8> = Bits::from_lit::<0>();
                loop {
                    match stage {
                        Stage::Load => { product = a.read() * b.read(); c_latch = c.read(); stage = Stage::Mul; }
                        Stage::Mul  => { result = product.clone() + c_latch.clone(); stage = Stage::Out; }
                        Stage::Out  => { out.write(result.clone()); stage = Stage::Load; }
                    }
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(regs(src), ["c_latch", "product", "result", "stage"]);
    }

    /// The item-2 generalization: `mac_pipeline`'s registers are **born inside the
    /// loop** (`let` in the body) and live across *interior* awaits — the G6 slice's
    /// pre-loop-only criterion missed all three. Backward liveness catches them.
    #[test]
    fn mac_pipeline_interior_await_registers() {
        let src = r#"
            #[hardware(sequential)]
            async fn mac_pipeline(clk: Clock<C>, a: In<Bits<8>, C>, b: In<Bits<8>, C>, c: In<Bits<8>, C>, out: Out<Bits<8>, C>) {
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
        assert_eq!(regs(src), ["c_s", "product", "sum"]);
    }

    /// A pre-loop constant only *read* in the loop is a wire, not a register —
    /// excluded because it has no def site inside the loop (even though it is live
    /// across ticks).
    #[test]
    fn constant_excluded() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, o: Out<Bits<32>, C>) {
                let mask = Bits::from_u32(7);
                let mut state = Bits::from_u32(1);
                loop {
                    state = state ^ mask;
                    o.write(state);
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(regs(src), ["state"]);
    }

    /// A same-cycle combinational temp (`let t = …; use t; tick`) is redefined at
    /// the loop head before any post-tick use, so it is killed and not a register.
    #[test]
    fn combinational_temp_excluded() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                let mut acc = Bits::from_u32(0);
                loop {
                    let t = a.read() + acc;
                    o.write(t);
                    acc = t;
                    clk.tick().await;
                }
            }
        "#;
        // `acc` crosses the tick (register); `t` is same-cycle (wire).
        assert_eq!(regs(src), ["acc"]);
    }

    // ── pre-tick alignment hazard (PRETICK_ALIGNMENT_GUARDRAIL.md) ──────────

    /// Every case below cites a variant that was **measured** — run in the simulator
    /// and independently under Verilator on its own transpiled SV. `flag` mirrors the
    /// measured verdict, so this table is the rule's evidence, not its restatement.
    fn hazard(src: &str) -> Vec<String> {
        Cfg::build(&parse(src)).map(|c| c.unprotected_pretick_out_write()).unwrap_or_default()
    }

    /// V1 — no `In` read anywhere; register assigned pre-tick; plain `Out`. DIVERGES.
    #[test]
    fn hazard_v1_assign_then_write_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                let mut r: Bits<8> = Bits::zero();
                loop { r = r + Bits::from_lit::<1>(); o.write(r); clk.tick().await; }
            }
        "#;
        assert_eq!(hazard(src), ["o"]);
    }

    /// V7 — the assigned register is never read back in the segment, and it still
    /// diverges. The rule must not require an in-segment read-back.
    #[test]
    fn hazard_v7_no_readback_still_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                let mut r: Bits<8> = Bits::zero();
                let mut s: Bits<8> = Bits::zero();
                loop { r = r + Bits::from_lit::<1>(); o.write(s); clk.tick().await; s = r; }
            }
        "#;
        assert_eq!(hazard(src), ["o"]);
    }

    /// V4 — an `In` read PRECEDES the assignment, installing the barrier. AGREES.
    #[test]
    fn hazard_v4_leading_read_not_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                let mut r: Bits<8> = Bits::zero();
                loop { r = r + i.read(); o.write(r); clk.tick().await; }
            }
        "#;
        assert!(hazard(src).is_empty());
    }

    /// V5 — the read comes AFTER the assignment, so it does not protect it. DIVERGES.
    /// This is what makes `leading_read_reaches` (a comb-path query) the right test
    /// rather than "the module reads an input somewhere".
    #[test]
    fn hazard_v5_trailing_read_still_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                let mut r: Bits<8> = Bits::zero();
                loop {
                    r = r + Bits::from_lit::<1>();
                    o.write(r);
                    let _late = i.read();
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(hazard(src), ["o"]);
    }

    /// V6 — the safe order: the register is assigned POST-tick. AGREES.
    #[test]
    fn hazard_v6_post_tick_assign_not_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                let mut r: Bits<8> = Bits::zero();
                loop { o.write(r); clk.tick().await; r = r + Bits::from_lit::<1>(); }
            }
        "#;
        assert!(hazard(src).is_empty());
    }

    /// W4 — MIXED alignment: a read on one arm, an unprotected assignment on the
    /// other. Measured to DIVERGE, which refuted the "any read in the module
    /// protects it" hypothesis.
    #[test]
    fn hazard_w4_mixed_alignment_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                let mut phase: u8 = 0;
                let mut r: Bits<8> = Bits::zero();
                loop {
                    if phase == 0 { r = i.read(); phase = 1; }
                    else { r = r + Bits::from_lit::<1>(); phase = 0; }
                    o.write(r);
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(hazard(src), ["o"]);
    }

    /// W8 — EXACTLY W4 with the output declared `RegOut`. Measured to AGREE. This
    /// pair is the whole reason the rule keys on the output write: `RegOut` commits
    /// at the edge, so the write's phase is unobservable.
    #[test]
    fn hazard_w8_regout_immunises_mixed_alignment() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, i: In<Bits<8>, C>, o: RegOut<Bits<8>, C>) {
                let mut phase: u8 = 0;
                let mut r: Bits<8> = Bits::zero();
                loop {
                    if phase == 0 { r = i.read(); phase = 1; }
                    else { r = r + Bits::from_lit::<1>(); phase = 0; }
                    o.write(r);
                    clk.tick().await;
                }
            }
        "#;
        assert!(hazard(src).is_empty(), "RegOut must be exempt — it commits at the edge");
    }

    /// W9 — EXACTLY V1 with the output declared `RegOut`. Measured to AGREE.
    #[test]
    fn hazard_w9_regout_immunises_the_minimal_case() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, i: In<Bits<8>, C>, o: RegOut<Bits<8>, C>) {
                let mut r: Bits<8> = Bits::zero();
                loop { r = r + Bits::from_lit::<1>(); o.write(r); clk.tick().await; }
            }
        "#;
        assert!(hazard(src).is_empty());
    }

    /// `probe_fsm` — measured to diverge, and measured to be the SAME defect as V1
    /// (adding a leading read on every path fixes it). Plain `Out`, so flagged.
    #[test]
    fn hazard_probe_fsm_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, inp: In<Bits<8>, C>, out: Out<Bits<8>, C>) {
                let mut phase: u8 = 0;
                let mut x: Bits<8> = Bits::from_lit::<0>();
                loop {
                    if phase == 0 { x = inp.read(); phase = 1; }
                    else { out.write(x.clone()); phase = 0; }
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(hazard(src), ["out"]);
    }

    /// `lfsr`-shaped: the guard reads inputs before every assignment, so the whole
    /// segment is barrier-pinned. AGREES — and it must, it is an equivalence-tested
    /// corpus module.
    #[test]
    fn hazard_lfsr_shape_not_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, reset_i: In<Logic, C>, yumi_i: In<Logic, C>, o: Out<Bits<32>, C>) {
                let mut state = Bits::from_u32(1);
                loop {
                    if reset_i.read().as_bool() { state = Bits::from_u32(1); }
                    else if yumi_i.read().as_bool() { state = state >> 1; }
                    o.write(state);
                    clk.tick().await;
                }
            }
        "#;
        assert!(hazard(src).is_empty());
    }

    /// `branch_merge_explicit` — drives three plain `Out`s from an unprotected path,
    /// but every write is the CONSTANT `Logic::One`. Measured to AGREE: the shift
    /// changes *when* the write happens, which is unobservable when the value does
    /// not depend on a register. Witness for the "write must read a register" clause.
    #[test]
    fn a_conditional_constant_write_is_flagged() {
        // This is `branch_merge_explicit`, and it was pinned here as a module the rule
        // must NOT flag — "a constant write is idempotent across the phase shift".
        //
        // MEASURED FALSE, 2026-08-25, by the corpus differential sweep: it leads its
        // own emitted SystemVerilog by exactly one cycle. The premise holds only when
        // the write happens on EVERY path; here the `pc == 1` arm writes `tail_o` and
        // the `pc == 0` arm may not, so the alternative is the port's HELD value and
        // *when* the write lands is observable. Both traces are pinned in
        // sequential_forwarding_divergence.rs (`pc_arm_write`, `pc_arm_toggle`) and
        // written up as 5.5 of the guardrail.
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, sel: In<Logic, C>, head_o: Out<Logic, C>, mid_o: Out<Logic, C>, tail_o: Out<Logic, C>) {
                let mut pc: u8 = 0;
                loop {
                    match pc {
                        0u8 => {
                            head_o.write(Logic::One);
                            if sel.read() == Logic::One { pc = 1; }
                            else { mid_o.write(Logic::One); tail_o.write(Logic::One); pc = 0; }
                        }
                        1u8 => { tail_o.write(Logic::One); pc = 0; }
                        _ => {}
                    }
                    clk.tick().await;
                }
            }
        "#;
        assert!(
            !hazard(src).is_empty(),
            "the conditionally-written constant is the measured divergence — not \
             flagging it lets a design through that disagrees with its own hardware"
        );
    }

    /// An UNCONDITIONAL constant write stays exempt, which is what keeps the rule off
    /// the corpus: the value is the same in either phase, so the shift is unobservable.
    #[test]
    fn an_unconditional_constant_write_is_not_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, sel: In<Logic, C>, o: Out<Logic, C>) {
                let mut pc: u8 = 0;
                loop {
                    o.write(Logic::One);
                    if sel.read() == Logic::One { pc = 1; } else { pc = 0; }
                    clk.tick().await;
                }
            }
        "#;
        assert!(
            hazard(src).is_empty(),
            "an unconditional constant write is idempotent across the phase shift — \
             flagging it would reject a correct design"
        );
    }

    /// A module with no `Out` at all cannot expose the hazard.
    #[test]
    fn hazard_regout_only_module_not_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, o: RegOut<Bits<8>, C>) {
                let mut r: Bits<8> = Bits::zero();
                loop { r = r + Bits::from_lit::<1>(); o.write(r); clk.tick().await; }
            }
        "#;
        assert!(hazard(src).is_empty());
    }

    /// The **back-edge clause** (added 2026-08-21). A local defined *post*-tick and
    /// read *pre*-tick is a flip-flop even though its live range crosses no tick: it
    /// crosses the loop back edge. The 2-FF synchronizer is the canonical case —
    /// under the tick-only rule this returned just `["ff1"]`, a silent
    /// under-approximation contradicted by the simulator's own behaviour, by
    /// independent hand-written Verilog, and by codegen (all three have two flops).
    /// See `Cfg::registers` for why `ff2` cannot be a wire.
    #[test]
    fn post_tick_def_read_pre_tick_is_a_register() {
        let src = r#"
            #[hardware(synchronizer)]
            async fn sync_2ff(clk: Clock<D>, d: In<Logic, S>, q: Out<Logic, D>) {
                let mut ff1 = Logic::Zero;
                let mut ff2 = Logic::Zero;
                loop {
                    q.write(ff2);
                    clk.tick().await;
                    ff2 = ff1;
                    ff1 = d.read();
                }
            }
        "#;
        assert_eq!(regs(src), ["ff1", "ff2"]);
    }

    /// The back-edge clause must not swallow the combinational case it neighbours:
    /// a temp defined *and* consumed within one post-tick segment dies there, so it
    /// is not live at the head and stays a wire (it is D-input logic in hardware).
    #[test]
    fn post_tick_temp_dying_in_its_own_segment_is_not_a_register() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                let mut acc = Bits::from_u32(0);
                loop {
                    o.write(acc);
                    clk.tick().await;
                    let t = a.read() + Bits::from_lit::<1>();
                    acc = t + acc;
                }
            }
        "#;
        // `acc` is carried across the boundary; `t` is consumed where it is born.
        assert_eq!(regs(src), ["acc"]);
    }

    /// Tuple-destructuring assignment `(phase, timer) = match …` (traffic_light):
    /// both targets are registers.
    #[test]
    fn tuple_assign_targets_are_registers() {
        let src = r#"
            #[hardware(sequential)]
            async fn tl(clk: Clock<C>, req: In<Logic, C>, r: Out<Logic, C>) {
                let mut phase = Phase::Green;
                let mut timer: u8 = 0;
                loop {
                    match phase { Phase::Green => { r.write(Logic::One); } _ => { r.write(Logic::Zero); } }
                    clk.tick().await;
                    (phase, timer) = match (phase, timer, req.read()) {
                        (Phase::Green, _, Logic::One) => (Phase::Yellow, 0),
                        (Phase::Yellow, t, _) if t < 1 => (Phase::Yellow, t + 1),
                        _ => (Phase::Green, 0),
                    };
                }
            }
        "#;
        assert_eq!(regs(src), ["phase", "timer"]);
    }

    /// A tick nested inside one branch, the register read after the merge: `state`
    /// is live across the interior tick (det_010 shape).
    #[test]
    fn branch_tick_state_register() {
        let src = r#"
            #[hardware(sequential)]
            async fn det(clk: Clock<C>, rstn: In<Logic, C>, in_i: In<Logic, C>, out_o: Out<Logic, C>) {
                let mut state = State::A;
                loop {
                    if rstn.read() == Logic::Zero { state = State::A; }
                    else { state = next(state, in_i.read()); }
                    clk.tick().await;
                    if matches!(state, State::D) { out_o.write(Logic::One); }
                    else { out_o.write(Logic::Zero); }
                }
            }
        "#;
        assert_eq!(regs(src), ["state"]);
    }

    // ── reachability well-formedness ────────────────────────────────────────

    fn check(src: &str) -> Result<(), String> {
        Cfg::build(&parse(src))
            .expect("has a loop")
            .check_reachability()
            .map_err(|(_, m)| m)
    }

    /// A branch that ticks with no matching else tick falls through to the loop
    /// head combinationally — a zero-time cycle. Rejected.
    #[test]
    fn tickless_fallthrough_rejected() {
        let src = r#"
            #[hardware(sequential)]
            async fn bad(clk: Clock<C>, cond: In<Logic, C>) {
                loop {
                    if cond.read() == Logic::One { clk.tick().await; }
                }
            }
        "#;
        assert!(check(src).is_err(), "a tickless fall-through path must be rejected");
    }

    /// An empty loop never ticks. Rejected.
    #[test]
    fn empty_loop_rejected() {
        let src = r#"
            #[hardware(sequential)]
            async fn spin(clk: Clock<C>) { loop {} }
        "#;
        assert!(check(src).is_err());
    }

    /// Uneven per-branch tick counts are legitimate: every path still crosses a
    /// tick, so the design is well-formed. This is the regression the plan calls
    /// out — the check must not reject asymmetric-but-sound designs.
    #[test]
    fn uneven_but_both_tick_accepted() {
        let src = r#"
            #[hardware(sequential)]
            async fn ok(clk: Clock<C>, cond: In<Logic, C>) {
                loop {
                    if cond.read() == Logic::One { clk.tick().await; }
                    else { clk.tick().await; clk.tick().await; }
                }
            }
        "#;
        assert!(check(src).is_ok(), "asymmetric-but-ticking branches are well-formed");
    }

    /// A trailing unconditional tick after a branch: both arms merge and reach the
    /// tick. Well-formed (mac_fsm / traffic_light shape).
    #[test]
    fn trailing_tick_after_branch_accepted() {
        let src = r#"
            #[hardware(sequential)]
            async fn ok(clk: Clock<C>, cond: In<Logic, C>, o: Out<Logic, C>) {
                loop {
                    if cond.read() == Logic::One { o.write(Logic::One); }
                    else { o.write(Logic::Zero); }
                    clk.tick().await;
                }
            }
        "#;
        assert!(check(src).is_ok());
    }

    /// A match where one arm ticks and another falls through is rejected.
    #[test]
    fn match_arm_without_tick_rejected() {
        let src = r#"
            #[hardware(sequential)]
            async fn bad(clk: Clock<C>, sel: In<Bits<2>, C>) {
                loop {
                    match sel.read() {
                        s if s == Bits::from_u32(0) => { clk.tick().await; }
                        _ => { }
                    }
                }
            }
        "#;
        assert!(check(src).is_err());
    }

    // ── nested-loop reachability (recursive) ────────────────────────────────

    /// The nested-loop builder's payoff: a tick inside one branch of a nested
    /// loop, with no tick on the other path — the inner loop spins without ticking
    /// when the condition holds. v1 folded the inner loop opaquely and *missed*
    /// this; the recursive sub-CFG check now rejects it.
    #[test]
    fn nested_loop_tickless_inner_cycle_rejected() {
        let src = r#"
            #[hardware(sequential)]
            async fn bad(clk: Clock<C>, c: In<Logic, C>) {
                loop {
                    loop {
                        if c.read() == Logic::One { clk.tick().await; }
                    }
                }
            }
        "#;
        assert!(check(src).is_err(), "a nested loop that can cycle without ticking must be rejected");
    }

    /// The canonical well-formed nested loop (rv32i / uart shape): tick first, then
    /// an exit test. Every inner iteration ticks before it can break.
    #[test]
    fn nested_loop_tick_then_break_accepted() {
        let src = r#"
            #[hardware(sequential)]
            async fn ok(clk: Clock<C>, ready: In<Logic, C>, o: Out<Logic, C>) {
                loop {
                    loop {
                        clk.tick().await;
                        if ready.read() == Logic::One { break; }
                    }
                    o.write(Logic::One);
                    clk.tick().await;
                }
            }
        "#;
        assert!(check(src).is_ok());
    }

    /// `break` before the tick must not read as a tickless cycle: the break path
    /// exits the loop (to the sink), it does not return to the inner head.
    #[test]
    fn nested_loop_break_before_tick_accepted() {
        let src = r#"
            #[hardware(sequential)]
            async fn ok(clk: Clock<C>, r: In<Logic, C>, o: Out<Logic, C>) {
                loop {
                    loop {
                        if r.read() == Logic::One { break; }
                        clk.tick().await;
                    }
                    o.write(Logic::One);
                    clk.tick().await;
                }
            }
        "#;
        assert!(check(src).is_ok());
    }

    /// The **zero-tick exit**. `outer`'s only clock boundary is `inner`, and `inner`
    /// is the mandated test-before-tick shape — so when `b` is high on entry it
    /// breaks without ticking and `outer` returns to its top in zero time.
    ///
    /// Measured before this was rejected: the simulator livelocks on it (99.5% CPU,
    /// no progress) while the flattened FSM runs one cycle per iteration. The
    /// optimistic fold accepted it because a tick-bearing nested loop contributed a
    /// Tick edge unconditionally.
    #[test]
    fn nested_loop_that_can_exit_before_ticking_is_not_a_clock_boundary() {
        let src = r#"
            #[hardware(sequential)]
            async fn bad(clk: Clock<C>, a: In<Logic, C>, b: In<Logic, C>) {
                loop {
                    loop {
                        if a.read() == Logic::One { break; }
                        loop {
                            if b.read() == Logic::One { break; }
                            clk.tick().await;
                        }
                    }
                }
            }
        "#;
        let err = check(src).expect_err(
            "a loop whose only boundary is a nested loop that can break before ticking \
             is a zero-time cycle — the simulator livelocks on it",
        );
        assert!(
            err.contains("left before it ticks"),
            "the diagnostic must name the nested loop, not advise adding a tick to a \
             body that already has one: {}",
            err
        );
    }

    /// The same zero-tick exit one level up: the module's own loop has no tick of its
    /// own and the `loop` it delegates to can break immediately.
    #[test]
    fn a_lone_nested_wait_is_not_a_clock_boundary_for_the_module_loop() {
        let src = r#"
            #[hardware(sequential)]
            async fn bad(clk: Clock<C>, go: In<Logic, C>) {
                loop {
                    loop {
                        if go.read() == Logic::One { break; }
                        clk.tick().await;
                    }
                }
            }
        "#;
        assert!(check(src).is_err());
    }

    /// A `while` needs no `break` to exit in zero iterations, so it is never a
    /// guaranteed boundary either. The transpiler desugars a tick-bearing `while`
    /// to `loop { if !cond { break; } … }`, which reaches the same verdict by the
    /// `break`-before-tick route — the two front-ends must not disagree.
    #[test]
    fn a_lone_while_wait_is_not_a_clock_boundary() {
        let src = r#"
            #[hardware(sequential)]
            async fn bad(clk: Clock<C>, go: In<Logic, C>) {
                loop {
                    while go.read() == Logic::One {
                        clk.tick().await;
                    }
                }
            }
        "#;
        assert!(check(src).is_err());
    }

    /// The counted case the fold's optimism exists for, and which keeps it: a `for`
    /// runs its body, so it ticks. This is `uart_tx`'s bit-timing delay, and the
    /// shape cause K ultimately has to support — it must stay well-formed.
    #[test]
    fn a_counted_for_is_still_a_clock_boundary() {
        let src = r#"
            #[hardware(sequential)]
            async fn ok(clk: Clock<C>, o: Out<Logic, C>) {
                loop {
                    o.write(Logic::One);
                    for _ in 0..8 {
                        clk.tick().await;
                    }
                }
            }
        "#;
        assert!(
            check(src).is_ok(),
            "a counted repetition runs its body and therefore ticks; rejecting it \
             would break uart_tx and rv32i_cpu"
        );
    }

    /// A wait that can exit early is fine as long as the *enclosing* loop ticks on
    /// its own — the zero-tick exit only matters when it is the only boundary. This
    /// is the common `waiter` idiom and must not be caught by the tightening.
    #[test]
    fn an_early_exiting_wait_is_fine_when_the_outer_loop_ticks_too() {
        let src = r#"
            #[hardware(sequential)]
            async fn ok(clk: Clock<C>, go: In<Logic, C>, o: Out<Logic, C>) {
                loop {
                    loop {
                        if go.read() == Logic::One { break; }
                        clk.tick().await;
                    }
                    o.write(Logic::One);
                    clk.tick().await;
                }
            }
        "#;
        assert!(check(src).is_ok());
    }

    /// `continue` that skips the only tick creates a tickless inner cycle.
    #[test]
    fn nested_loop_continue_before_tick_rejected() {
        let src = r#"
            #[hardware(sequential)]
            async fn bad(clk: Clock<C>, c: In<Logic, C>) {
                loop {
                    loop {
                        if c.read() == Logic::One { continue; }
                        clk.tick().await;
                    }
                }
            }
        "#;
        assert!(check(src).is_err());
    }

    /// A `while` whose body has a tickless path is caught by the recursion, even
    /// though the *outer* loop is well-formed (the while is folded as ticking there).
    #[test]
    fn nested_while_tickless_path_rejected() {
        let src = r#"
            #[hardware(sequential)]
            async fn bad(clk: Clock<C>, cond: In<Logic, C>, c: In<Logic, C>) {
                loop {
                    while cond.read() == Logic::One {
                        if c.read() == Logic::One { clk.tick().await; }
                    }
                    clk.tick().await;
                }
            }
        "#;
        assert!(check(src).is_err());
    }

    /// A tick-free nested loop is combinational (an unrolled `for`) and is NOT
    /// subject to the must-tick rule — its `body → head` is not a clocked cycle.
    #[test]
    fn tick_free_nested_loop_accepted() {
        let src = r#"
            #[hardware(sequential)]
            async fn ok(clk: Clock<C>, o: Out<Bits<8>, C>) {
                loop {
                    let mut acc = Bits::from_u32(0);
                    for i in 0..8 { acc = acc + Bits::from_u32(1); }
                    o.write(acc);
                    clk.tick().await;
                }
            }
        "#;
        assert!(check(src).is_ok());
    }

    // ── definite assignment (combinational modules only) ────────────────────

    fn da(src: &str) -> Result<(), String> {
        Cfg::build_combinational(&parse(src))
            .check_definite_assignment()
            .map_err(|(_, m)| m)
    }

    /// A combinational output assigned on all paths (both `if` arms) is fine.
    #[test]
    fn comb_output_all_paths_ok() {
        let src = r#"
            #[hardware(combinational)]
            fn m(sel: In<Logic, ()>, o: Out<Logic, ()>) {
                if sel.read() == Logic::One { o.write(Logic::One); }
                else { o.write(Logic::Zero); }
            }
        "#;
        assert!(da(src).is_ok());
    }

    /// A straight-line unconditional write is fine.
    #[test]
    fn comb_output_unconditional_ok() {
        let src = r#"
            #[hardware(combinational)]
            fn m(a: In<Logic, ()>, b: In<Logic, ()>, o: Out<Logic, ()>) {
                let p = a.read() & b.read();
                o.write(p);
            }
        "#;
        assert!(da(src).is_ok());
    }

    /// A combinational output written in only one branch (no else) infers a latch.
    #[test]
    fn comb_output_partial_is_latch() {
        let src = r#"
            #[hardware(combinational)]
            fn m(sel: In<Logic, ()>, o: Out<Logic, ()>) {
                if sel.read() == Logic::One { o.write(Logic::One); }
            }
        "#;
        assert!(da(src).is_err(), "a partial combinational output must be flagged as a latch");
    }

    /// A `match` missing a write on one arm infers a latch.
    #[test]
    fn comb_output_partial_match_is_latch() {
        let src = r#"
            #[hardware(combinational)]
            fn m(sel: In<Bits<2>, ()>, o: Out<Logic, ()>) {
                match sel.read() {
                    s if s == Bits::from_u32(0) => { o.write(Logic::One); }
                    _ => {}
                }
            }
        "#;
        assert!(da(src).is_err());
    }

    /// An output never written is not a latch here (it may be driven by a
    /// submodule instantiation) — only a *partial* write is flagged.
    #[test]
    fn comb_output_never_written_not_flagged() {
        let src = r#"
            #[hardware(combinational)]
            fn m(a: In<Logic, ()>, o: Out<Logic, ()>) {
                let _ = a.read();
            }
        "#;
        assert!(da(src).is_ok());
    }

    /// A sequential `Out` written conditionally is an enabled register (holds),
    /// NOT a latch — definite-assignment must not fire on sequential modules
    /// (verified `sim ≡ BaseJump` on `bsg_dff_en`).
    #[test]
    fn sequential_conditional_output_not_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn dff_en(clk: Clock<C>, d: In<Bits<8>, C>, en: In<Logic, C>, q: Out<Bits<8>, C>) {
                loop {
                    clk.tick().await;
                    if en.read() == Logic::One { q.write(d.read()); }
                }
            }
        "#;
        // The public router skips sequential modules entirely.
        assert!(crate::check_definite_assignment(&parse(src)).is_ok());
    }

    // ── read-timing classification (item 3) ─────────────────────────────────

    use ReadTiming::{Deferred, Immediate};

    fn timings(src: &str) -> Vec<ReadTiming> {
        crate::classify_reads(&parse(src))
    }

    /// Loop-top reads before a tick are Deferred (register at the edge). `dff_en`'s
    /// `en`/`d` reads follow the tick with no further tick before close → Immediate
    /// (they consume the value the edge produced — the enabled-register idiom).
    #[test]
    fn trailing_reads_are_immediate() {
        let src = r#"
            #[hardware(sequential)]
            async fn dff_en(clk: Clock<C>, d: In<Bits<8>, C>, en: In<Logic, C>, q: Out<Bits<8>, C>) {
                loop {
                    clk.tick().await;
                    if en.read() == Logic::One { q.write(d.read()); }
                }
            }
        "#;
        // Source order: en, d — both after the only tick, none follows → Immediate.
        assert_eq!(timings(src), [Immediate, Immediate]);
    }

    /// `mac_pipeline`: three loop-top reads (`a`, `b`, `c`) all precede ticks →
    /// Deferred. No reads follow the last tick.
    #[test]
    fn loop_top_reads_are_deferred() {
        let src = r#"
            #[hardware(sequential)]
            async fn mac_pipeline(clk: Clock<C>, a: In<Bits<8>, C>, b: In<Bits<8>, C>, c: In<Bits<8>, C>, out: Out<Bits<8>, C>) {
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
        assert_eq!(timings(src), [Deferred, Deferred, Deferred]);
    }

    /// `traffic_light`: `request.read()` is the trailing next-state read — after the
    /// tick, nothing follows before the iteration closes → Immediate.
    #[test]
    fn traffic_light_request_is_immediate() {
        let src = r#"
            #[hardware(sequential)]
            async fn tl(clk: Clock<C>, request: In<Logic, C>, r: Out<Logic, C>) {
                let mut phase = Phase::Green;
                let mut timer: u8 = 0;
                loop {
                    match phase { Phase::Green => { r.write(Logic::One); } _ => { r.write(Logic::Zero); } }
                    clk.tick().await;
                    (phase, timer) = match (phase, timer, request.read()) {
                        (Phase::Green, _, Logic::One) => (Phase::Yellow, 0),
                        _ => (Phase::Green, 0),
                    };
                }
            }
        "#;
        assert_eq!(timings(src), [Immediate]);
    }

    /// `det_010` canonical: `rstn`/`in_i` are read before the tick → Deferred.
    #[test]
    fn det_010_canonical_reads_deferred() {
        let src = r#"
            #[hardware(sequential)]
            async fn det_010(clk: Clock<C>, rstn: In<Logic, C>, in_i: In<Logic, C>, out_o: Out<Logic, C>) {
                let mut state = State::A;
                loop {
                    if rstn.read() == Logic::Zero { state = State::A; }
                    else { state = match (state, in_i.read()) { _ => State::A, }; }
                    clk.tick().await;
                    if matches!(state, State::D) { out_o.write(Logic::One); }
                    else { out_o.write(Logic::Zero); }
                }
            }
        "#;
        // rstn (outer cond, branches tick) and in_i (match scrutinee, tick follows).
        assert_eq!(timings(src), [Deferred, Deferred]);
    }

    /// The item-3 target: every `In` read in `det_010_awaits` — including the
    /// `while in_i.read() == 0 { tick }` condition and the nested-`if` reads — is
    /// Deferred, because a tick lives after each within its branch/loop. This is the
    /// classification the runtime heuristic could not compute.
    #[test]
    fn det_010_awaits_all_reads_deferred() {
        let src = r#"
            #[hardware(sequential)]
            async fn det_010_awaits(clk: Clock<C>, rstn: In<Logic, C>, in_i: In<Logic, C>, out_o: Out<Logic, C>) {
                loop {
                    out_o.write(Logic::Zero);
                    if rstn.read() == Logic::Zero {
                        out_o.write(Logic::Zero);
                        clk.tick().await;
                    } else if in_i.read() == Logic::Zero {
                        clk.tick().await;
                        while in_i.read() == Logic::Zero {
                            clk.tick().await;
                        }
                        if in_i.read() == Logic::One {
                            clk.tick().await;
                            if in_i.read() == Logic::Zero {
                                out_o.write(Logic::One);
                                clk.tick().await;
                            }
                        }
                    } else {
                        clk.tick().await;
                    }
                }
            }
        "#;
        // Source order: rstn, in_i(else-if cond), in_i(while cond), in_i(if), in_i(inner if).
        assert_eq!(timings(src), [Deferred, Deferred, Deferred, Deferred, Deferred]);
    }

    /// `sipo_block`: the loop-top read and all three mid-phase reads precede a tick
    /// (the final tick follows the last read) → all Deferred.
    #[test]
    fn sipo_block_mid_phase_reads_deferred() {
        let src = r#"
            #[hardware(sequential)]
            async fn sipo(clk: Clock<C>, data_i: In<Bits<4>, C>, data_o: RegOut<Bits<16>, C>) {
                loop {
                    let w0 = data_i.read();
                    clk.tick().await;
                    let w1 = data_i.read();
                    clk.tick().await;
                    let w2 = data_i.read();
                    clk.tick().await;
                    let w3 = data_i.read();
                    data_o.write(pack(w0, w1, w2, w3));
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(timings(src), [Deferred, Deferred, Deferred, Deferred]);
    }

    /// A same-cycle double-read of one port (both before the tick) → both Deferred,
    /// classified independently by position (no shared tracker needed anymore).
    #[test]
    fn multiple_reads_same_port_before_tick() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                loop {
                    let x = a.read();
                    let y = a.read();
                    o.write(x + y);
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(timings(src), [Deferred, Deferred]);
    }

    /// No `In` parameters → nothing to classify.
    #[test]
    fn no_in_params_empty() {
        let src = r#"
            #[hardware(sequential)]
            async fn free(clk: Clock<C>, o: Out<Bits<8>, C>) {
                let mut v = Bits::from_u32(0);
                loop { o.write(v); clk.tick().await; v = v + Bits::from_u32(1); }
            }
        "#;
        assert_eq!(timings(src), Vec::<ReadTiming>::new());
    }

    // ── memory staging rules ────────────────────────────────────────────────

    fn mem_check(src: &str) -> Result<(), String> {
        Cfg::build(&parse(src))
            .expect("has a loop")
            .check_memory_staging()
            .map_err(|(_, m)| m)
    }

    /// A ROM read, wrapped in a body one line away from the linear form: the
    /// trailing `for` adds two clock boundaries and nothing else, and it is what
    /// makes `control_extract` rewrite the module. Under the old segment-based
    /// check that rewrite collapsed the staging and the observation into one
    /// segment and the module was refused — the false positive this rule exists to
    /// fix. `tests/extracted_memory_equivalence.rs` carries the behavioural half.
    #[test]
    fn a_counted_pause_after_the_observation_is_not_a_same_cycle_read() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: RegOut<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    rom.read_port::<0>().read(a.read().as_usize());
                    clk.tick().await;
                    o.write(rom.read_port::<0>().data());
                    for _ in 0..2 { clk.tick().await; }
                }
            }
        "#;
        assert_eq!(mem_check(src), Ok(()));
    }

    /// The same shape without the pause — the linear form, which always worked.
    #[test]
    fn a_read_observed_after_the_tick_is_accepted() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: RegOut<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    rom.read_port::<0>().read(a.read().as_usize());
                    clk.tick().await;
                    o.write(rom.read_port::<0>().data());
                }
            }
        "#;
        assert_eq!(mem_check(src), Ok(()));
    }

    /// The rule itself, in a module control extraction rewrites: the observation
    /// is in the staging cycle, so it reads the result an edge early. The old
    /// check could not see this at all once the ticks became `pc` states.
    #[test]
    fn a_same_cycle_observation_is_rejected_even_when_extracted() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: RegOut<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    rom.read_port::<0>().read(a.read().as_usize());
                    o.write(rom.read_port::<0>().data());
                    clk.tick().await;
                    for _ in 0..2 { clk.tick().await; }
                }
            }
        "#;
        let err = mem_check(src).expect_err("observing in the staging cycle must be rejected");
        assert!(err.contains("is read before the `clk.tick().await`"), "{err}");
    }

    /// Observing a port nothing ever stages.
    #[test]
    fn an_unstaged_port_is_rejected() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: RegOut<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    rom.read_port::<0>().read(a.read().as_usize());
                    clk.tick().await;
                    o.write(rom.read_port::<1>().data());
                }
            }
        "#;
        let err = mem_check(src).expect_err("an unstaged port never becomes ready");
        assert!(err.contains("nothing stages a `read()`"), "{err}");
    }

    /// Two stagings of one bus that a single cycle really does reach — one after
    /// the other, no edge between them. There is one address bus, so the design
    /// has to say which address; the simulator would silently keep the last.
    #[test]
    fn two_stagings_of_one_bus_in_one_cycle_are_rejected() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: RegOut<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    rom.read_port::<0>().read(a.read().as_usize());
                    rom.read_port::<0>().read(0);
                    clk.tick().await;
                    o.write(rom.read_port::<0>().data());
                }
            }
        "#;
        let err = mem_check(src).expect_err("one address bus, two drivers in a cycle");
        assert!(err.contains("accessed 2 times in one cycle"), "{err}");
    }

    /// Stagings on **exclusive** branches are a multiplexer, not a conflict: no
    /// path joins them, so no cycle drives the bus twice, and the emitted
    /// `always_comb` assigns the address inside the arm that runs. Counting sites
    /// per phase instead reported a design error on `rv32i_cpu`'s seven regfile
    /// writebacks, which are exactly this shape.
    /// `tests/extracted_memory_equivalence.rs::exclusive_arm_writes_…` is the
    /// behavioural half.
    #[test]
    fn stagings_on_exclusive_branches_are_a_mux() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, s: In<Logic, C>, a: In<Bits<8>, C>, o: RegOut<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    if s.read() == Logic::One {
                        rom.read_port::<0>().read(a.read().as_usize());
                    } else {
                        rom.read_port::<0>().read(0);
                    }
                    clk.tick().await;
                    o.write(rom.read_port::<0>().data());
                }
            }
        "#;
        assert_eq!(mem_check(src), Ok(()));
    }

    /// Stagings in *different* cycles are the ordinary multi-phase pattern — the
    /// address bus is phase-gated — and must not be read as a conflict.
    #[test]
    fn stagings_in_different_cycles_are_not_a_conflict() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, b: In<Bits<8>, C>, o: RegOut<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    rom.read_port::<0>().read(a.read().as_usize());
                    clk.tick().await;
                    o.write(rom.read_port::<0>().data());
                    rom.read_port::<0>().read(b.read().as_usize());
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(mem_check(src), Ok(()));
    }

    /// A read port and a write port are different buses, so one of each in a cycle
    /// is the baseline RAM shape (`dual_port_ram`, every memory fixture).
    #[test]
    fn a_read_and_a_write_in_one_cycle_are_different_buses() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, we: In<Logic, C>, a: In<Bits<8>, C>, d: In<Bits<16>, C>,
                       o: RegOut<Bits<16>, C>) {
                let mem = Memory::<Bits<16>, 1, 1, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    if we.read() == Logic::One {
                        mem.write_port::<0>().write(a.read().as_usize(), d.read());
                    }
                    mem.read_port::<0>().read(a.read().as_usize());
                    clk.tick().await;
                    o.write(mem.read_port::<0>().data());
                }
            }
        "#;
        assert_eq!(mem_check(src), Ok(()));
    }

    /// The trailing statements of a body run in the same cycle as the head — the
    /// back edge costs nothing — so a staging in one and an observation in the
    /// other is a same-cycle read. The segment-index form of this rule got the
    /// pairing wrong here in both directions.
    #[test]
    fn a_trailing_staging_and_a_head_observation_share_a_cycle() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: RegOut<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    o.write(rom.read_port::<0>().data());
                    clk.tick().await;
                    rom.read_port::<0>().read(a.read().as_usize());
                }
            }
        "#;
        let err = mem_check(src).expect_err("the trailing staging shares the head's cycle");
        assert!(err.contains("is read before the `clk.tick().await`"), "{err}");
    }

    fn plain_out_ports(src: &str) -> Vec<String> {
        Cfg::build(&parse(src))
            .expect("has a loop")
            .memory_result_drives_plain_out()
    }

    /// The measured divergence: an extracted module's plain `Out` wired straight to
    /// a read result lands a cycle late in SystemVerilog. `vlir_lower`'s copy of
    /// this rule counts LOWERED phases and an extracted module has one, so it sees
    /// nothing.
    #[test]
    fn a_plain_out_from_a_read_result_across_phases_is_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: Out<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    rom.read_port::<0>().read(a.read().as_usize());
                    clk.tick().await;
                    o.write(rom.read_port::<0>().data());
                    for _ in 0..2 { clk.tick().await; }
                }
            }
        "#;
        assert_eq!(plain_out_ports(src), ["o"]);
    }

    /// Through a same-cycle local, which is the same net wearing a name.
    #[test]
    fn the_drive_is_followed_through_a_same_cycle_local() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: Out<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    rom.read_port::<0>().read(a.read().as_usize());
                    clk.tick().await;
                    let w = rom.read_port::<0>().data();
                    o.write(w);
                    for _ in 0..2 { clk.tick().await; }
                }
            }
        "#;
        assert_eq!(plain_out_ports(src), ["o"]);
    }

    /// A register between the result and the port is the supported form (`mp_reg`),
    /// and a register is exactly what a value that survives an edge is.
    #[test]
    fn a_register_between_the_result_and_the_port_is_not_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: Out<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                let mut q: Bits<16> = Bits::zero();
                loop {
                    o.write(q);
                    rom.read_port::<0>().read(a.read().as_usize());
                    clk.tick().await;
                    q = rom.read_port::<0>().data();
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(plain_out_ports(src), Vec::<String>::new());
    }

    /// A single-phase module: the post-tick segment shares the head's phase, so the
    /// port is driven from the captured word in the cycle it was captured — correct,
    /// and pinned behaviourally as `rom_direct`.
    #[test]
    fn a_single_phase_module_is_not_flagged() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: Out<Bits<16>, C>) {
                let rom = Memory::<Bits<16>, 1, 0, C, 1, 1>::new(clk.clone(), 256);
                loop {
                    rom.read_port::<0>().read(a.read().as_usize());
                    clk.tick().await;
                    o.write(rom.read_port::<0>().data());
                }
            }
        "#;
        assert_eq!(plain_out_ports(src), Vec::<String>::new());
    }

    /// A module with no `Memory` local has nothing to check — and a `read_port`
    /// method on something else is not a memory access.
    #[test]
    fn a_module_without_a_memory_is_unaffected() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, a: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                loop {
                    o.write(a.read());
                    clk.tick().await;
                }
            }
        "#;
        assert_eq!(mem_check(src), Ok(()));
    }
}
