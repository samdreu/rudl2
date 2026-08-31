# The received-`Memory` port ABI

**Status: DECIDED AND LANDED 2026-08-27** (user-directed; cause P in `TODO`).
Verified end to end by `tests/received_memory_abi.rs` — the simulator running a
real `Memory` object and the transpiled child instantiated under a hand-written
owner both reproduce the trace derived from the memory windows.

## 1. The question, and the fork that was recorded

`#[hardware]` accepts a `Memory<…>` **parameter** (2026-08-26): a module may be
HANDED its storage instead of declaring it, and the simulator runs it — but the
transpiler had no answer for what a received memory *is* in SystemVerilog. The
recorded fork (do not pick by accident): **(a)** a set of address/data/enable
PORTS on the module with the array living in the parent, or **(b)** a hierarchy
edge (item 4's structural-module territory). The user ruled for **(a)**.

## 2. The cut, and why it is where it is

The declared-memory lowering already had a natural bus inside it: per accessed
port, combinational nets (`<m>_rd<i>_addr`, `<m>_wr<j>_{en,addr,data}`), a
child-side read-capture pipeline (`_q<k>`/`_v<k>` registers), and — on the other
side — the array, its preload, the continuous read
(`assign <m>_rd<i>_data = mem[…]`) and the guarded non-blocking commit. The ABI
cuts exactly there:

* **The child keeps every timing decision.** The staging `always_comb` and the
  capture registers clock the child's own edge, so the emitted body is the
  declared-memory body minus the array — the same nets, now ports. The capture
  register clocks the same edge on either side of the cut, so the semantics are
  identical to a declared memory (verified).
* **The owner provides what any RAM wrapper provides**, nothing more:

  ```systemverilog
  assign  <m>_rd<i>_data = mem[<m>_rd<i>_addr];                           // continuous read
  always_ff @(posedge clk) if (<m>_wr<j>_en) mem[<m>_wr<j>_addr] <= <m>_wr<j>_data;
  ```

  With that pair, a same-edge read/write collision at one address captures the
  OLD word — **ReadFirst**, matching the simulator's default. The collision
  policy is the owner's (the `.write_first()` builder configures the owner's
  object; a WriteFirst owner adds the forwarding mux on its side).

## 3. The contract

For each **used** port of a received `Memory<T, R, W, D, READ_LAT, WRITE_LAT>`
parameter named `m`:

| port | dir | width |
|---|---|---|
| `m_rd<i>_addr` | output | `M_ADDR_W` |
| `m_rd<i>_data` | input | `width(T)` |
| `m_wr<j>_en` | output | 1 |
| `m_wr<j>_addr` | output | `M_ADDR_W` |
| `m_wr<j>_data` | output | `width(T)` |

* **`M_ADDR_W` is a module parameter** (default 1): the depth is a runtime
  argument of the owner's constructor and is not in the type, so the child
  cannot size the address bus — the instantiating context supplies it, exactly
  like every other generic width (the harness's `with_params` mechanism).
* **Used ports only.** An unused port of a multi-port memory gets no nets
  internally and no bus externally — an undriven output port would be a lint
  error, not an interface.
* **No read enable on the bus.** A continuous array read needs none; the
  child's own valid pipeline (driven by the internal `_en` net) handles
  `is_ready()`.
* **`WRITE_LAT` must be 1** for now: the bus carries the freshly-staged write
  nets, and at deeper write latency the committing value is a child-side
  pipeline register — exposing the committing stage is a straightforward
  extension, refused honestly until built and verified. Any `READ_LAT ≥ 1`
  works (the capture chain is child-side and feeds off the data input).

## 4. Implementation notes

* `chir_lower::received_memory_decls` parses the parameter type and pushes a
  `CHIRMemoryDecl { received: true, … }`, so the body lowering (staging,
  capture, checks — `copper_analysis::memory_locals` already collects
  parameters) is byte-for-byte the declared path.
* At VLIR: `lower_mem_decls` skips the array/preload/read-assign,
  `mem_write_commits` skips the commit, `lower_to_vlir` synthesizes the bus
  ports and the `M_ADDR_W` parameter, and `mem_net_defaults` sizes address
  defaults parametrically (`'0`).
* Two integration traps found while landing, both now handled: the emitter's
  wire-declaration collector must not redeclare a bus net that is now a port
  (double declaration), and `drop_unread_wires` must treat bus outputs as
  **externally read** — the array and commit moved across the boundary, so the
  dead-wire eliminator otherwise deletes the bus's `always_comb` defaults (it
  did, on landing day).

## 5. What this did and did not unblock

`rv32i_cpu_pipelined` no longer refuses on its `Memory` parameter (cause P is
discharged); it now surfaces its own next recorded blockers (the struct-typed
pipeline latches / tuple-returning EX stage — see its `build.rs` SKIP entry).
The sweep still cannot generate a case for a memory-receiving module (the
harness would have to synthesize an owner); `tests/received_memory_abi.rs` is
the dedicated gate for the ABI itself.
