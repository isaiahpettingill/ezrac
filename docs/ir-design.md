# EZRA HIR and TBIR Design

This document specifies EZRA's main compiler intermediate representations. The goal is a complete design that fits EZRA's real use case: target-specific low-level programs, strong hardware-aware diagnostics, safe optimization, readable assembly, and reusable shared libraries for algorithms and math.

EZRA does not require a generic backend-neutral IR. Full applications and games are expected to be compiled for one selected target. Cross-platform EZRA code is expected mostly in shared libraries that avoid target-specific hardware behavior.

## Pipeline

```text
source
  -> pest parse tree
  -> AST
  -> typed HIR
  -> target-bound IR (TBIR)
  -> machine lowering
  -> EZRA target assembly
  -> metadata-generated target assembler
  -> configured binary layout emitter
  -> final binary/package
```

HIR and TBIR may be represented in memory as Rust structs. A serialized IR cache may be binary. Textual IR is useful for debugging and tests, but it is not the required primary format. If binary IR is used, the compiler must still provide an inspectable dump format and preserve source locations for diagnostics.

## Design Goals

- Detect memory, pointer, port, layout, and target ABI errors before running.
- Preserve enough source structure for diagnostics, inlining, tail-call analysis, and loop analysis.
- Support basic and target-aware optimizations without relying on undefined behavior.
- Model retro hardware directly: 8-bit, 16-bit, and 24-bit values and addresses are normal.
- Keep volatile memory, port I/O, inline assembly, interrupts, and target SDK calls explicit.
- Lower predictably to readable EZRA target assembly.
- Support shared libraries by checking them in HIR and binding them to a target when used.

## Non-Goals

- EZRA does not need LLVM-like generic portability as the primary IR purpose.
- EZRA does not assume full apps are cross-platform.
- EZRA does not optimize by assuming C-style undefined behavior.
- EZRA does not hide ports or MMIO behind normal memory operations.
- EZRA does not require textual IR as the canonical storage format.

## HIR

HIR is the typed high-level representation produced after AST construction. It is mostly target-independent, though it may retain target conditions and feature constraints for later binding.

HIR owns:

- resolved names, imports, modules, aliases, and visibility
- typed declarations, statements, and expressions
- constant values and target-independent range facts
- source locations for all diagnostics
- shared-library validation before final target binding
- source-shaped control flow for high-quality diagnostics
- function purity/effect summaries when target-independent
- recursion, tail-recursion, and tail-call candidate markings
- loop candidate markings for TBIR passes

HIR may perform conservative target-independent optimizations:

- constant folding
- constant propagation for pure constants
- dead constant branch removal
- simple pure expression simplification
- marking functions as inline candidates
- marking unreachable source paths for diagnostics

HIR must not perform optimizations that require selected target memory regions, port maps, MMIO maps, pointer width, ABI details, or target cache/layout facts.

## Shared Libraries

Shared libraries are checked in HIR. They should avoid assumptions that only make sense for one hardware target unless guarded by future conditional compilation or target-specific modules.

When a target-specific app uses a shared library, the library's HIR is instantiated into the app's TBIR using the selected target profile. Final pointer-width checks, memory model diagnostics, ABI checks, and target-aware optimizations happen after this binding.

## TBIR

TBIR is the target-bound checked optimization IR. It is created after target selection and project layout resolution.

TBIR owns:

- selected target pointer width and address width
- native and legal integer widths
- target ABI and calling convention facts
- concrete memory regions and permissions
- concrete sections and object placement intent
- port maps, port widths, and port directions
- MMIO regions and volatility rules
- SDK ABI metadata
- inline assembly effects and clobbers
- interrupt and naked-function constraints
- target optimization profile

TBIR should be structured enough for loop and tail-call optimization, and explicit enough for later machine lowering. A practical shape is structured control regions plus explicit basic blocks, with lowering to pure basic blocks before machine lowering if needed.

## Values and Types

TBIR values are typed with explicit widths and signedness:

```text
u8, i8, u16, i16, u24, i24, bool
ptr<space, T>
array<T, N>
struct S
```

Pointer width is selected by target. Integer widths are semantic widths, not host-machine widths. Operations must encode EZRA's defined behavior directly.

Arithmetic operations should distinguish signedness and behavior:

```text
add.u8.wrap
sub.u16.wrap
mul.u24.wrap
div.u16.zero_on_zero
mod.i24.zero_on_zero
cmp.lt.i16
cmp.lt.u16
```

This keeps optimizations and lowering from relying on undefined behavior.

## Memory Model

The target profile defines memory regions. Regions include start address, size, permissions, volatility defaults, executable/data status, and optional cache/layout properties.

Example:

```text
region ram {
  start: 0x040000
  size:  0x040000
  access: read/write
}

region rom {
  start: 0x010000
  size:  0x010000
  access: read/execute
}

region vram {
  start: 0x080000
  size:  0x040000
  access: volatile read/write
}
```

TBIR memory operations name width, address, memory space or object, volatility, and source location.

```text
%x = load.u8 object @_player_x
store.u8 object @_player_x, %value
%status = load.u8 volatile region vram, 0x080010
```

## Pointer Provenance and Bounds

TBIR should track pointer provenance where possible.

Pointer knowledge classes:

```text
ObjectPointer {
  object: global/local/embed/stack object
  offset: constant or range
  length: known
}

RegionPointer {
  region: target memory region
  address: constant or range
  length: known or unknown
}

UnknownPointer {
  pointee type known
  target pointer width known
}
```

Diagnostics:

- hard error for statically proven out-of-bounds object pointer access
- hard error for address outside selected target address space
- hard error for writes through pointers known to target read-only regions
- hard error for invalid section/object placement or overlap
- warning or note when pointer provenance is lost and bounds cannot be proven, if useful

The compiler should not reject every unknown pointer. Explicit absolute pointers, casts, SDK boundaries, and inline assembly can lose provenance. The key requirement is to reject proven invalid behavior and preserve enough information for optimization when available.

## Ports and MMIO

Ports are not memory. TBIR port operations are explicit:

```text
%key = port.read.u8 keyboard_status
port.write.u8 vdp_data, %byte
```

The target port map defines valid ports, width, direction, volatility, and optional symbolic names. Diagnostics should reject invalid widths, invalid directions, unavailable ports, and values outside the port width when known.

MMIO is memory with volatility and region constraints. MMIO loads/stores must not be reordered around other volatile, port, asm, or unknown-effect operations unless a target-specific rule explicitly permits it.

## Effects Model

Every TBIR operation has an effect summary.

```text
pure
read(object/region)
write(object/region)
volatile_read(region)
volatile_write(region)
port_read(port)
port_write(port)
call(effect summary)
asm(clobbers/effects)
control
```

Optimizations may remove, combine, or reorder operations only when the effect model, alias/provenance analysis, and target rules prove it safe.

Inline assembly is an opaque effectful operation with typed inputs, typed outputs, clobbers, memory effects, port effects, and flags effects. `asm volatile` must be preserved and ordered according to its declared effects.

## Control Flow

HIR keeps source-shaped control flow. TBIR should preserve structured loops long enough to run loop and tail-recursion passes, then may lower to explicit basic blocks.

Terminators include:

- return
- branch
- conditional branch
- loop backedge
- tail call when not yet rewritten
- trap or target-defined termination if introduced later

TBIR should preserve source locations through transformations, including inlining and tail-recursion conversion.

## Tail Calls and Recursion

HIR detects recursion, tail recursion, and tail-call candidates. TBIR decides legality using target ABI facts.

Tail-call optimization is legal when:

- the call is in tail position
- caller and callee calling conventions are compatible
- return value representation is compatible
- no required cleanup remains after the call
- interrupt/naked/ABI constraints permit the rewrite

Tail recursion can usually be rewritten into a loop even when general sibling-call optimization is not supported. The rewrite must preserve arithmetic behavior, effects, and source diagnostics.

## Loop Optimizations

HIR marks loop candidates and may remove loops with constant false conditions. TBIR performs target-aware loop optimizations.

Supported optimization families:

- loop-invariant code motion
- induction variable simplification
- strength reduction
- unrolling when code-size policy allows
- nested loop reordering when dependence analysis proves legality
- loop tiling/blocking when the target has cache or memory-layout facts that justify it
- bounds-check simplification when pointer/range analysis proves safety

Loop reordering and tiling are legal only when:

- iteration dependence analysis proves the new order equivalent
- no volatile, port, inline asm, interrupt-visible, or unknown-call effects are reordered incorrectly
- pointer aliasing does not create store/load conflicts
- arithmetic overflow behavior remains equivalent
- the target optimization profile says locality/cache transformation is useful

## Inlining

HIR marks explicit and likely inline candidates. TBIR makes final inlining decisions using target cost models.

Inlining inputs:

- `inline` modifier
- function size
- call frequency if known
- target call cost
- code-size policy
- stack/register pressure estimate when available
- effect summary
- recursion and tail-call interactions

Inlining must preserve diagnostics and source locations. It must not hide target diagnostics caused by the inlined body.

## Integer Optimization and Legalization

TBIR uses range analysis and target integer facts to optimize integer usage.

Examples:

- narrow temporaries when range analysis proves high bits unused
- avoid widening on targets where narrow ops are cheaper
- select `u8`, `u16`, or `u24` address arithmetic forms based on pointer width and range
- replace multiply/divide by constants with shifts/adds when equivalent under EZRA semantics
- choose runtime helpers for operations that are not efficient or legal natively

Integer transformations must preserve signedness, wrap behavior, and divide/remainder-by-zero behavior.

## Target Optimization Profile

Each target should provide optimization facts.

```text
pointer_width: 24
native_ints: [8, 16, 24]
prefer_code_size: true
has_cache: false
cache_line_size: none
call_cost: medium
unroll_threshold: small
loop_tiling: disabled
```

Targets with caches can enable locality optimizations:

```text
has_cache: true
cache_line_size: 32
prefer_data_locality: true
loop_tiling: enabled
```

No cache-oriented optimization should run merely because it is generally known. It must be enabled by target facts and proven legal.

## Machine Lowering

Machine lowering converts optimized TBIR into target instruction choices, registers, stack slots, concrete calling convention operations, helper calls, and readable EZRA target assembly.

Machine lowering owns:

- register selection and constraints
- stack frame layout
- parameter and return passing
- helper ABI calls
- concrete branch forms
- target instruction choice
- target assembly emission

TBIR remains above machine lowering. It should not hard-code exact register allocation, but it may know target register classes and constraints for cost modeling.

## Assembler and Binary Layout

The assembler is target-specific and generated from metadata. It accepts documented EZRA assembly syntax for that target, encodes instructions, resolves symbols, applies relocations when supported, and emits sections plus symbol/map information.

The binary layout emitter consumes assembled sections, symbols, target profile data, and project configuration to produce the final artifact shape:

- raw `.bin`
- Agon MOS executable wrapper
- future ROM/cart/tape/disk/calculator packages
- maps and symbol tables

Instruction encoding and binary/container packaging are separate responsibilities.

## Diagnostics Enabled by TBIR

TBIR should enable diagnostics that are difficult or impossible in AST-only code:

- static out-of-bounds pointer access
- pointer crossing object or region boundaries
- address outside target address space
- write to read-only memory or section
- invalid volatile/MMIO access
- invalid port direction or width
- section does not fit in region
- section/object overlap
- target ABI mismatch
- unavailable target instruction or inline asm form
- tail-call candidate rejected with reason when requested by diagnostics mode

Diagnostics must point back to original source locations even after HIR/TBIR transformations.
