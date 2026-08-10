# EZRA IR Design

This document describes EZRA's internal HIR/TBIR pipeline and EZIR, its public interchange format. `ezrac emit-ir --stage hir|tbir` prints internal debugging dumps. `ezrac emit-ir --stage ezir` writes stable EZIR v1 JSON that can be compiled with `ezrac build-ir`.

EZRA does not require a generic backend-neutral IR. Full applications and games are expected to be compiled for one selected target. Cross-platform EZRA code is expected mostly in shared libraries that avoid target-specific hardware behavior.

## Pipeline

The current source path is:

```text
source
  -> pest parse tree
  -> AST
  -> HIR
  -> TBIR
  -> EZIR boundary
  -> target source emitter
  -> target assembler
  -> configured binary/package emitter
  -> final artifact
```

HIR currently retains typed declarations, function bodies, and lightweight analysis such as recursion, tail-call, and loop-candidate markings. TBIR binds the selected target and layout, records memory regions plus typed global/MMIO/embed object provenance, applies scalar simplification, typed immutable-local propagation, local common-subexpression cleanup, pure scalar loop-invariant hoisting, and named global-read LICM, expands approved explicit-inline calls, decides safe tail calls, rewrites supported direct tail recursion into loops, and supplies the transformed program to every source emitter. Target emitters then select native arithmetic and shift instructions for their CPU. The shared intrinsic catalog and zero/one/two-result source nodes are implemented, but target lowering remains uneven. TBIR is not yet a fully lowered basic-block or register-allocation IR.

HIR and TBIR remain internal Rust structs and may change at any time. Their text dumps are for debugging and tests. EZIR v1 owns its serialized types and is the compatibility boundary; consumers must not depend on the Rust representation.

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

## EZIR v1

EZIR is the public, target-bound interchange format built from the optimized TBIR program. HIR stays internal because its bodies and types remain tied to EZRA analysis and syntax.

EZIR v1 is readable JSON with an explicit top-level `version`. Serialized enums have a `kind` field and snake-case names. The required module contains target requirements and declarations. The optional `metadata` map does not affect code generation.

The target block records:

- address width
- pointer address width
- pointer storage width
- native integer widths
- whether the module requires port I/O

Pointer address width and storage width are separate. For example, MSP430X uses a 20-bit pointer address width and 32-bit storage width. `build-ir` rejects a selected target whose widths, native integers, or port-I/O support do not meet the module requirements.

EZIR owns its declaration, type, expression, place, statement, operator, and inline-assembly types. It does not serialize `ast::Program`, HIR, source spans, source units, or optimization reports. Source text and string metadata are optional.

Current v1 declarations are source-shaped and use structured control flow. SSA is not required. Imports should be resolved before EZIR emission. Bank placement wrappers are preserved.

```sh
ezrac emit-ir --stage ezir --target agonlight-mos-ez80 main.ezra > main.ezir
ezrac build-ir --target agonlight-mos-ez80 main.ezir
```

Unsupported versions, duplicate symbols, duplicate fields or parameters, invalid widths, and incompatible targets are rejected with diagnostics. Compatible v1 additions must use optional fields or metadata. A change that alters required fields or existing semantics requires a new version.

The Rust types in `src/ezir.rs` are an implementation detail. The JSON field names and documented behavior are the contract.

## TBIR

TBIR is the internal target-bound checked optimization IR. It is created after target selection and project layout resolution.

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

Pointer width is selected by target. Integer widths are semantic widths, not host-machine widths. Operations must encode EZRA's defined behavior directly. The intrinsic catalog can validate source types independently of a target, but target lowering checks legal scalar widths, address widths, instruction forms, and ABI resources; unsupported combinations diagnose rather than silently changing width.

### Intrinsic result lists and the catalog

Intrinsic results are stored as ordered scalar result lists in HIR/TBIR. A result list has zero, one, or two entries; it is not a tuple type and cannot be used as an aggregate expression. The catalog records each operation's canonical name and aliases, argument/result counts, memory read/write effect, volatile policy, overlap rule, and compile-time bit-index or bit-range requirements. The source catalog is documented in `docs/language.md`.

User functions use the same zero/one/two result shape. A two-result call must feed a matching two-place binding or a matching return; arrays, structs, `bytes`, strings, tuples, and other aggregate or large values remain pointer-passed.

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

TBIR memory operations name width, address, memory space or object, volatility, and source location. Intrinsic block operations also carry their source/destination range relationship and explicit endian mode.

```text
%x = load.u8 object @_player_x
store.u8 object @_player_x, %value
%status = load.u8 volatile region vram, 0x080010
%copy = mem.move nonvolatile %destination, %source, %length
```

`copy_nonoverlapping` has a must-not-overlap rule and diagnoses statically proven overlap. `move` has overlap-safe copy semantics. Endian load/store intrinsics name little- or big-endian byte order rather than inheriting target order. These general memory intrinsics require ordinary nonvolatile memory; `mem.peek8` and `mem.poke8` are the scalar-access exceptions and preserve one byte access for volatile/MMIO use.

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

MMIO is memory with volatility and region constraints. MMIO loads/stores must not be reordered around other volatile, port, asm, or unknown-effect operations unless a target-specific rule explicitly permits it. Intrinsics that may combine or repeat memory accesses reject statically known volatile/MMIO operands; they do not imply safe device-register read-modify-write. Target SDK functions remain the right place for device-specific access sequences.

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

After target lowering, the eZ80 backend may run a conservative basic-block CFG
cleanup. It starts at `__ezra_start` and explicit public, banked, interrupt,
naked, and inline-assembly roots, removes blocks with no control-flow path to a
root, and removes branches whose target is the next block. Programs containing
inline assembly skip this cleanup because assembly can enter or branch to labels
outside the structured IR.

## Tail Calls and Recursion

HIR detects recursion, tail recursion, and tail-call candidates. TBIR decides legality using target ABI facts.

Tail-call optimization is legal when:

- the call is in tail position
- caller and callee calling conventions are compatible
- return value representation is compatible
- no required cleanup remains after the call
- interrupt/naked/ABI constraints permit the rewrite

Tail recursion can usually be rewritten into a loop even when general sibling-call optimization is not supported. The rewrite must preserve arithmetic behavior, effects, and source diagnostics.

The implemented eZ80-family pass rewrites direct self calls in tail position when all arguments use the register ABI. It evaluates arguments once from left to right into temporary locals before assigning parameters, so dependent arguments behave as simultaneous assignments. Calls in existing nested loops, interrupt or naked functions, argument-slot ABIs, and calls with more than three parameters keep the normal call path.

TBIR also approves sibling tail calls when caller and callee are normal eZ80-family functions with matching return types and register-only arguments. The eZ80 emitter treats that approval as a lowering instruction and emits a jump after normal ordered argument evaluation. Rejected calls remain ordinary calls.

## Loop Optimizations

HIR marks loop candidates and may remove loops with constant false conditions. TBIR performs target-aware loop optimizations.

The scalar loop pass hoists nontrivial pure scalar local initializers into a loop preheader when all dependencies are defined outside and unchanged by the loop. The memory pass can also hoist expressions rooted in named globals when the object has a known writable, nonvolatile region and the loop contains no same-object write or unknown alias barrier. TBIR records each object’s type, address, size, region, access, and volatility. Alias checks are deliberately whole-object: any write to the same global blocks all reads from it. Calls, ports, MMIO, embeds, read-only or volatile regions, dereferences, address-taking, banked pointers, inline assembly, unknown writes, and loops with explicit exits remain barriers. Pointer-range analysis, caching, unrolling, tiling, and locality transforms remain deferred.

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

The implemented pass supports explicitly marked straight-line value and void functions. TBIR rejects unsupported return shapes, naked and interrupt functions, and functions in direct or mutual recursion cycles. Dumps record each applied or rejected decision and its reason. The eZ80 emitter expands only the functions approved by TBIR; it no longer makes inline policy decisions.

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
- target-local temporary allocation and safe reuse of proven nonvolatile
  absolute local storage loads
- stack frame layout
- parameter and return passing
- zero/one/two-result ABI placement
- helper ABI calls. eZ80 helpers are emitted on demand from a dependency graph;
  a helper is not emitted merely because another helper exists.
- concrete branch forms
- target instruction choice
- target assembly emission

A two-result ABI is target-owned. Current implementations include a normal first result plus caller-provided hidden storage for the second result on eZ80-family and AVR paths, `R0`/`R1` on TMS9900, and `A`/`EX` on DCPU-16. Some targets lower paired intrinsic calls but reject user-defined two-result functions. Extern assembly declarations are accepted only when the selected backend has a matching result convention; otherwise the compiler diagnoses the declaration or call.

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
