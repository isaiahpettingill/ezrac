# Optimization Safety Review

This review classifies the candidate passes in `optimizations.md` against EZRA's
low-level hardware semantics. No pass is accepted unless it can preserve volatile
memory ordering, port I/O ordering, inline-asm barriers, defined arithmetic
behavior, and emulator-backed test behavior.

## Accepted

- Constant folding for pure scalar expressions.
- Idempotent bit-operation cleanup (`x & x` and `x | x` become `x`) only when
  `x` is a pure scalar expression. Calls, memory reads, ports, pointers, and
  inline assembly are not rewritten because evaluating them fewer times can
  change behavior.
- Dead branch elimination when the condition is a compile-time constant and the
  removed branch has no reachable side effects.
- Unreachable statement elimination after terminators, provided removed
  statements are unreachable in source semantics.
- Peephole cleanup for exact duplicate register loads and register copies when
  neither the source nor destination register changed. Non-Z80 backends remove
  only adjacent duplicate register copies and self-copies whose instruction does
  not change flags. Memory operands and inline assembly are excluded. The eZ80
  pass tracks register state across safe generated instructions and may also reuse a
  cached load from a proven nonvolatile absolute local/global range across
  register-only instructions; stores, indirect memory, ports, calls, branches,
  and inline assembly invalidate the cache.
- Lowered CFG cleanup that removes only blocks unreachable from `__ezra_start`
  and explicit public, banked, interrupt, naked, or inline-assembly roots. It
  keeps programs containing inline assembly opaque because assembly may enter
  or branch to labels the compiler cannot model, and it removes a jump only
  when both outcomes reach the same immediately following label.
- Target-specific multiply lowering when the selected CPU supports the emitted
  instruction sequence and emulator tests cover the result.
- Explicit function inlining in TBIR when it approves a source-shaped body outside
  direct or mutual recursion, naked functions, interrupt functions, inline assembly,
  and unsupported control exits. Typed argument temporaries evaluate expressions
  once from left to right; parameters and locals are renamed. Calls in short-circuit
  right-hand sides and `while` conditions stay as calls when moving their prefix
  statements would change conditional evaluation.
- Direct tail-recursion conversion in TBIR. New argument values are evaluated left
  to right into typed temporaries before parameters are assigned, and the pass does
  not rewrite calls inside existing loops.
- Sibling tail calls between compatible register-ABI eZ80-family functions with
  matching return types and no interrupt, naked, argument-slot, or stack cleanup.
- Immutable-local scalar propagation when the initializer contains only literals,
  immutable locals or parameters, unary/binary operations, and casts. Substitutions
  retain the local's declared type so width, signedness, and aliases still drive
  validation and instruction selection. Calls, ports, memory, addresses, pointers,
  and aggregate values are excluded.
- Local common-subexpression cleanup inside straight-line blocks for pure scalar
  expressions. Assignments, control-flow joins, loops, calls, ports, and inline
  assembly clear the available-expression set.
- Pure scalar loop-invariant initializer hoisting when every dependency is defined
  outside and unchanged by the loop. Loops with exits and initializers involving
  calls, ports, pointers, aggregates, or inline assembly are rejected.
- Named global-read loop-invariant hoisting when TBIR proves the object is in a
  known writable, nonvolatile region and the loop has no same-object write or
  unknown alias barrier. Alias checks are conservative at whole-object granularity.
  MMIO, embeds, read-only or volatile regions, calls, ports, dereferences,
  address-taking, banked pointers, inline assembly, and unknown writes block it.

## Rejected

- Reordering or coalescing port reads/writes.
- Reordering or eliminating volatile memory accesses.
- Moving operations across `asm volatile`.
- Moving memory operations across inline asm with `clobber memory`.
- Moving port operations across inline asm with `clobber ports`.
- Replacing divide/remainder by zero with traps or host-language behavior; EZRA
  runtime semantics return zero for runtime division/modulo by zero.
- Assuming signed division can use host/platform rounding if it does not truncate
  toward zero.

## Needs Design

- General constant propagation across memory reads. This needs alias analysis and
  must treat volatile memory and memory-clobbering asm as barriers.
- Copy propagation for memory values or locals that may alias through pointers.
  Pure immutable scalar locals are supported.
- Pointer-derived and range-sensitive memory LICM. Named global reads use typed
  object provenance and whole-object alias checks; field/element range overlap,
  escaped pointers, and unknown provenance remain conservative.
- Cache-aware loop tiling and memory caching. Current targets expose cache facts,
  but no transform runs without a target benefit model and dependence proof.
- General automatic or cost-based function inlining. Explicitly marked
  source-shaped functions are handled by TBIR. MOS6502 and TMS9900 retain a
  narrow target-specific size optimization for unannotated zero-argument compact
  wrappers, but broader effect-aware policy still needs design.
- Loop-invariant code motion. This needs explicit effect modeling and must not
  move reads from ports or volatile memory.
- Stack traffic reduction around calls, interrupts, naked functions, and inline
  asm clobbers.
- Block-copy lowering. `ldir`/`otir`-style lowering is target-specific and must
  preserve volatile and overlap behavior.

## Root and helper rules

- Public functions and public constants, globals, and embeds are executable or
  storage roots. Banked declarations, interrupt functions, naked functions,
  `main`, exact function labels named by inline assembly, and programs with
  extern assembly are roots as well. Private declarations remain removable
  when no reachable code or root references them.
- eZ80 runtime helpers are selected from a fixed dependency graph after code
  emission. A helper is emitted only when generated or inline assembly refers
  to it; signed 24-bit multiplication also retains its unsigned 24-bit helper
  dependency. This selection happens before section output, not by pruning a
  monolithic helper block after emission.

## Required Regression Coverage

- A pure optimized case and a side-effecting non-optimized case for each pass.
- Emulator tests for volatile memory ordering, port output ordering, and inline
  asm memory/port clobber barriers.
- CLI artifact tests for optimized builds, so map/bin behavior is not changed by
  optimizer-only refactors.
