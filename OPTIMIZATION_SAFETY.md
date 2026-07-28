# Optimization Safety Review

This review classifies the candidate passes in `optimizations.md` against EZRA's
low-level hardware semantics. No pass is accepted unless it can preserve volatile
memory ordering, port I/O ordering, inline-asm barriers, defined arithmetic
behavior, and emulator-backed test behavior.

## Accepted

- Constant folding for pure scalar expressions.
- Dead branch elimination when the condition is a compile-time constant and the
  removed branch has no reachable side effects.
- Unreachable statement elimination after terminators, provided removed
  statements are unreachable in source semantics.
- Peephole cleanup for exact duplicate register loads with no intervening
  memory, port, call, or inline-asm effects.
- Target-specific multiply lowering when the selected CPU supports the emitted
  instruction sequence and emulator tests cover the result.
- Explicit function inlining when TBIR approves a straight-line body outside
  direct or mutual recursion, naked functions, and interrupt functions. Argument
  expressions are evaluated once before the body is expanded.
- Direct tail-recursion conversion for register-ABI eZ80-family functions. New
  argument values are evaluated left to right into temporaries before parameters
  are assigned, and the pass does not rewrite calls inside existing loops.
- Sibling tail calls between compatible register-ABI eZ80-family functions with
  matching return types and no interrupt, naked, argument-slot, or stack cleanup.

Follow-up implementation issues: #15, #16.

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
- Copy propagation for locals that may alias through pointers.
- Automatic or cost-based function inlining. Explicitly marked straight-line
  functions are supported, but broader effect-aware policy still needs design.
- Loop-invariant code motion. This needs explicit effect modeling and must not
  move reads from ports or volatile memory.
- Stack traffic reduction around calls, interrupts, naked functions, and inline
  asm clobbers.
- Block-copy lowering. `ldir`/`otir`-style lowering is target-specific and must
  preserve volatile and overlap behavior.

## Required Regression Coverage

- A pure optimized case and a side-effecting non-optimized case for each pass.
- Emulator tests for volatile memory ordering, port output ordering, and inline
  asm memory/port clobber barriers.
- CLI artifact tests for optimized builds, so map/bin behavior is not changed by
  optimizer-only refactors.
