# Copper Verilog/SystemVerilog Output Standards

## Purpose

This document defines the required standards for RTL emitted by Copper transpilation.

Scope:
- Production transpilation output (not test-only helper outputs).
- SystemVerilog-first emission policy.

## Normative Language

- MUST: mandatory requirement.
- SHOULD: recommended unless there is a documented reason not to.
- MAY: optional behavior.

## 1) Target Language and Compatibility

- Emission target MUST be SystemVerilog by default.
- Emission SHOULD remain compatible with common open-source and commercial toolchains where practical.
- Tool-profile-specific restrictions MAY be applied in legalization (for example, stricter profiles for Yosys/Verilator).

## 2) Semantic Source of Truth

- Copper simulator behavior is the semantic source of truth.
- Any mismatch between Copper simulator traces and emitted RTL behavior MUST be treated as a transpiler bug until proven otherwise.

## 3) Timing Model Representation

- Internal scheduling MUST preserve explicit pre-edge and post-edge regions before final RTL emission.
- Emitted RTL MUST preserve this scheduling intent (no emitter-level reinterpretation).

## 4) Assignment Policy

Assignment intent is decided in scheduling and preserved through emission.

- Sequential architectural state updates MUST use non-blocking assignment semantics.
- Combinational logic MUST use blocking assignment semantics (or continuous assignment where applicable).
- A given architectural signal MUST NOT be driven by both combinational and sequential contexts.
- A given architectural signal MUST NOT mix blocking and non-blocking assignment semantics.
- Emitter MUST be mechanical with respect to scheduled intent.

## 5) Reset and Initialization

- Production RTL MUST NOT rely on `initial` blocks.
- Deterministic startup behavior MUST be expressed via reset semantics.
- Test-only/simulation-only initialization MAY exist in test profiles, but MUST be excluded from production transpilation mode.

## 6) Width and Signedness

- Width/signedness inference MUST be strict.
- Ambiguous inference MUST produce a hard compile error.
- Silent truncation/extension/coercion MUST NOT occur.
- Any required cast, extension, or truncation MUST be explicit in IR and reflected in emitted RTL.

## 7) Unsupported Rust Constructs

- Unsupported constructs MUST fail fast with compile errors.
- Diagnostics MUST include:
  - source span,
  - reason for rejection,
  - suggested rewrite pattern.
- Transpiler MUST NOT silently degrade semantics for unsupported constructs.

## 8) Hierarchy Handling

Transpilation SHOULD support three hierarchy modes:

- Preserve hierarchy
- Full flatten
- Hybrid/selective flatten

Default behavior SHOULD be preserve-first or hybrid, with profile-driven flattening when required.

## 9) Memory Handling Modes

Transpilation SHOULD support staged memory capability modes:

- No-memory lowering mode
- Minimal memory subset mode
- Explicit memory IR lowering mode

Milestone rollout MAY begin with no/minimal memory mode, then advance to explicit memory IR mode.

## 10) Determinism and Reproducibility

For identical input and configuration, emitted RTL SHOULD be deterministic.

- Ordering of modules/ports/declarations SHOULD be stable.
- Temporary naming SHOULD be stable.
- Output MUST NOT include nondeterministic host/time noise.

## 11) Readability and Structure

- Generated RTL SHOULD be readable and reviewable.
- Correctness/legalization and prettification SHOULD remain separate concerns.
- Naming SHOULD avoid reserved keyword collisions.

## 12) Extensibility Model

- Compiler architecture SHOULD use an internal pass pipeline.
- Public plugin/extension APIs MAY be deferred until IR and pass contracts stabilize.

## 13) Compliance Expectations

A transpilation change is considered standards-compliant only if it:

- preserves simulator-equivalent behavior,
- satisfies assignment/timing/reset/width rules above,
- and passes configured validation gates (lint/sim/equivalence).

## PR Review Checklist

Use this checklist for transpilation-related pull requests.

| Item | Pass/Fail | Notes |
|---|---|---|
| Emission target is SystemVerilog-first and profile-appropriate |  |  |
| Copper simulator trace remains semantic source of truth for changed behavior |  |  |
| Pre-edge/post-edge timing intent is explicit and preserved through lowering |  |  |
| Assignment policy holds (`<=` for sequential architectural updates, blocking/continuous for combinational) |  |  |
| No mixed combinational/sequential driving of the same architectural signal |  |  |
| No mixed blocking/non-blocking assignment semantics on the same architectural signal |  |  |
| Production output does not rely on `initial` blocks |  |  |
| Reset behavior is explicit and deterministic where required |  |  |
| Width/signedness inference remains strict (no silent coercions) |  |  |
| Unsupported Rust constructs fail with actionable diagnostics (span + reason + rewrite) |  |  |
| Hierarchy mode choice is explicit (preserve/flatten/hybrid) and justified |  |  |
| Memory mode choice is explicit (none/minimal/explicit-memory-IR) and justified |  |  |
| Emission output is deterministic for identical input/configuration |  |  |
| Names are legal and keyword collisions are handled |  |  |
| Validation gates pass (lint, simulation, equivalence where configured) |  |  |

### Suggested Evidence to Attach in PR Description

- Before/after simulator trace snippets for affected modules.
- Representative emitted RTL diff for one changed module.
- Validation command list and summarized results.
- Any intentional standards deviations with rationale and follow-up plan.
