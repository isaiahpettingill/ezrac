
# EZRA Phase 2 Optimization Spec

Status: candidate design notes. These optimizations are remaining work and must be evaluated before implementation. Do not treat this file as accepted compiler behavior until each pass has a clear IR dependency, a safety argument for volatile memory/port I/O/inline asm, and emulator-backed tests.

## 1. Goal

Phase 2 optimizations make generated EZRA assembly smaller and faster while preserving exact hardware semantics.

Primary targets:

```text
- constant folding
- dead code elimination
- branch layout
- eZ80 ADL-aware width lowering
- MLT-based multiplication lowering
- block-copy lowering
- stack traffic reduction
- peephole cleanup
- volatile/port/asm safety
```

Non-negotiable rule:

```text
Correctness beats cleverness.
```

EZRA must never optimize away or reorder:

```text
- port I/O
- volatile memory
- asm volatile
- inline asm with memory clobber
- inline asm with ports clobber
- test result writes
- test halt writes
```

---

## 2. Optimization Pipeline

Recommended compiler pipeline:

```text
source
  -> parse
  -> AST
  -> name resolution
  -> type checking
  -> typed IR
  -> high-level optimizations
  -> lowered EZRA IR
  -> low-level optimizations
  -> register/stack assignment
  -> eZ80 assembly
  -> assembly peephole
  -> assemble
  -> emulator test
```

### 2.1 High-level optimization passes

Run these before machine lowering:

```text
1. constant folding
2. constant propagation
3. copy propagation
4. dead branch elimination
5. dead code elimination
6. inline small functions
7. simplify boolean expressions
8. recognize memcpy/memset/fill loops
```

### 2.2 Low-level optimization passes

Run after typed IR is lowered toward eZ80:

```text
1. width narrowing
2. branch layout
3. stack traffic reduction
4. MLT multiply lowering
5. block-transfer lowering
6. port/mmio barrier validation
7. peephole assembly cleanup
```

### 2.3 Final validation

Every optimization pass must be covered by emulator-backed tests.

For each optimization, keep at least two tests:

```text
- optimized output gives correct result
- volatile/port/asm version is not incorrectly optimized
```

---

## 3. IR Requirements

The optimizer needs typed IR with explicit side effects.

Every instruction should carry:

```text
type width:
  u8/i8/u16/i16/u24/i24/bool/ptr<T>

effect:
  pure
  reads_memory
  writes_memory
  reads_volatile
  writes_volatile
  reads_port
  writes_port
  asm_volatile
  clobbers_memory
  clobbers_ports

branch likelihood:
  unknown
  likely_true
  likely_false
```

Example IR:

```text
%pad:u8 = in_port PAD1_LO              ; reads_port
%mask:u8 = and %pad, 0x10              ; pure
%cond:bool = ne %mask, 0               ; pure
br_if %cond, .pressed, .not_pressed
```

Port and volatile operations create ordering barriers.

---

## 4. Constant Folding

Fold all pure compile-time expressions.

Allowed:

```text
10 + 20              -> 30
0xFFu8 + 1u8         -> 0u8
1 << 8               -> 256
320 * 240            -> 76800
cast<u8>(300u16)     -> 44
```

Rules:

```text
- unsigned overflow wraps by type width
- signed overflow wraps by type width
- division by zero in constant expressions is a compile error
- shifts by width or greater are a compile error
- bool expressions fold to true/false
```

Do not fold across:

```text
- in PORT
- volatile memory load
- asm volatile
- unknown function call
```

Example:

```text
let x: u24 = 320 * 240
```

becomes:

```text
let x: u24 = 76800
```

---

## 5. Constant Propagation

Replace known local constants with their values.

Example:

```text
let mask: u8 = 0x10
if (pad & mask) != 0 { ... }
```

becomes:

```text
if (pad & 0x10) != 0 { ... }
```

Allowed for:

```text
- immutable locals
- constants
- pure expression results
```

Not allowed for:

```text
- volatile loads
- port reads
- memory loads unless alias-safe
- values modified by inline asm memory clobbers
```

Important:

```text
let a: u8 = in PAD1_LO
let b: u8 = in PAD1_LO
```

must not become:

```text
let a: u8 = in PAD1_LO
let b: u8 = a
```

because each port read is observable.

---

## 6. Copy Propagation

Replace trivial copies when safe.

Example:

```text
let a: u24 = player_x
let b: u24 = a
return b
```

becomes:

```text
return player_x
```

Safe only when:

```text
- source is pure
- source has not been overwritten
- no memory/asm clobber invalidates it
- source is not a volatile read
- source is not a port read
```

---

## 7. Dead Code Elimination

Remove code that cannot affect the result.

Remove:

```text
- unused pure temporary values
- unreachable blocks
- branches with constant false condition
- stores to locals never read again
```

Never remove:

```text
- port writes
- port reads
- volatile loads
- volatile stores
- asm volatile
- calls to functions marked effectful
- writes to TEST_RESULT / TEST_HALT
```

Example:

```text
let x: u24 = 1 + 2
test.pass()
```

If `x` is unused, remove it.

But:

```text
let x: u8 = in PAD1_LO
test.pass()
```

must keep the port read if it remains in source order and is not proven unused *and* allowed to be removed. Default EZRA rule: keep all port reads.

---

## 8. Function Inlining

Inline functions marked `inline`.

Example:

```text
inline fn pressed(pad: u16, button: u16) -> bool {
    return (pad & button) != 0
}
```

Call:

```text
if input.pressed(pad, BTN_A) { ... }
```

becomes:

```text
if (pad & BTN_A) != 0 { ... }
```

Rules:

```text
- inline only functions explicitly marked inline
- do not inline recursive functions
- do not inline naked functions
- do not inline interrupt functions
- do not inline functions containing raw asm unless marked inline_asm_safe
- preserve port/volatile ordering inside the function
```

Recommended default inline candidates:

```text
- input.pressed
- video.present
- debug.char
- small casts/helpers
- pointer offset helpers
```

Inlining should happen before constant folding is rerun.

---

## 9. Boolean Simplification

Simplify pure boolean expressions.

Examples:

```text
x == true       -> x
x == false      -> !x
!!x             -> x
if true {...}   -> unconditional block
if false {...}  -> remove block
```

Integer-to-bool is explicit in EZRA, so do not invent implicit conversions.

This is valid:

```text
if (pad & BTN_A) != 0 { ... }
```

This is invalid:

```text
if pad & BTN_A { ... }
```

---

## 10. Width Narrowing

The optimizer should use the smallest safe machine width.

EZRA types already make this easier:

```text
u8/i8      -> 8-bit operation
u16/i16    -> 16-bit operation where safe
u24/i24    -> 24-bit operation
ptr<T>     -> 24-bit pointer operation
```

### 10.1 Narrow pure arithmetic

If a value is declared `u8`, generate 8-bit code.

Example:

```text
let x: u8 = a + b
```

should use `A`-oriented 8-bit arithmetic when possible.

### 10.2 Avoid accidental 24-bit work

This is bad:

```text
; u8 addition accidentally widened to HL
ld hl, (_a)
ld de, (_b)
add hl, de
ld (_x), hl
```

This is better:

```text
ld a, (_a)
ld b, a
ld a, (_b)
add a, b
ld (_x), a
```

### 10.3 Use 16-bit operations only when upper byte is irrelevant

Inside ADL mode, `.S`-style 16-bit operation is allowed only when all of these are true:

```text
- operation is pure integer arithmetic, not pointer arithmetic
- result type is u16/i16 or narrower
- no memory address is formed by the register being shortened
- upper byte of the involved register is provably dead after the instruction
- no later code observes the full 24-bit register value
```

Do not use `.S`-style operation for:

```text
- ptr<T>
- ptr24
- address calculations
- stack pointer adjustments unless explicitly proven and tested
- memory loads/stores through HL/DE/BC
- values later passed as u24/i24/ptr24
```

Important hazard:

```text
LD.SIS HL, 3456h
```

may leave upper register/address behavior different from normal ADL operation. Use this only when the upper byte is dead or intentionally zero/ignored.

### 10.4 Default rule

Use normal ADL operations for `u24`, `i24`, and all pointers.

Use short-width code only for `u8/u16/i8/i16` values.

---

## 11. MLT-Based Multiplication

`MLT rr` is an 8×8→16 multiply.

It multiplies:

```text
rr.high * rr.low -> rr[15:0]
```

Supported operands:

```text
MLT BC
MLT DE
MLT HL
```

### 11.1 Direct lowering: u8 * u8 -> u16

Source:

```text
let z: u16 = cast<u16>(x_u8) * cast<u16>(y_u8)
```

Preferred lowering:

```text
ld h, x
ld l, y
mlt hl
; HL = x * y
```

or use `BC`/`DE` depending on register pressure.

Rule:

```text
If both multiplicands are known u8 and result is u16 or wider, use MLT.
```

### 11.2 u8 * constant

For constants:

```text
x * 0   -> 0
x * 1   -> x
x * 2   -> x << 1
x * 4   -> x << 2
x * 8   -> x << 3
x * 256 -> cast wider and shift
```

For non-power-of-two 8-bit constants, use either:

```text
- MLT if result width is u16/u24
- shift-add if cheaper by cost model
```

Example:

```text
x * 3 -> x + (x << 1)
```

### 11.3 u16 * u16

Because `MLT` is only 8×8→16, full `u16 * u16` requires partial products.

For `u16 * u16 -> u16` wrapping result:

```text
Let a = ah:al
Let b = bh:bl

low16 = (al * bl)
      + ((ah * bl) << 8)
      + ((al * bh) << 8)

Ignore (ah * bh) << 16 because result wraps to u16.
```

Use `MLT` for each 8×8 partial product.

For `u16 * u16 -> u24`:

```text
include lower 24 bits of all partial products
```

Runtime helper:

```text
__ezra_mul_u16
```

should be MLT-based.

### 11.4 u24 * u24

Full `u24 * u24 -> u24` wrapping result should be a runtime helper:

```text
__ezra_mul_u24
```

Use MLT-based partial products internally.

Optimization rules:

```text
x * 0   -> 0
x * 1   -> x
x * 2   -> x << 1
x * 4   -> x << 2
x * 8   -> x << 3
x * -1  -> 0 - x for signed
```

Do not inline full `u24 * u24` by default. It bloats code.

### 11.5 Signed multiply

Signed multiply should lower to helpers unless one side is a simple constant.

Helpers:

```text
__ezra_mul_i8
__ezra_mul_i16
__ezra_mul_i24
```

For signed multiply by power of two, shift only if the signed wrapping behavior is preserved.

---

## 12. Division and Modulo

There is no equivalent simple hardware divide instruction in the baseline EZRA target.

Rules:

```text
- division by constant power of two may become shift
- unsigned division by other constants may use strength reduction only if proven correct
- general division calls helper
- modulo by power of two may become mask for unsigned values
```

Examples:

```text
x / 2u8      -> x >> 1
x % 8u8      -> x & 7
x / 10u24    -> __ezra_div_u24(x, 10)
```

Signed division calls helpers unless the transform is trivially safe and tested.

---

## 13. Block Copy Lowering

Recognize copy loops and calls to `memcpy`.

### 13.1 Preferred primitive

Use `LDIR` for memory-to-memory copy.

In ADL mode:

```text
HL = source pointer
DE = destination pointer
BC = length
LDIR
```

Zilog documents `LDIR` as copying `(HL)` to `(DE)`, decrementing `BC`, incrementing `HL` and `DE`, and repeating until `BC == 0`; in ADL mode, `BC` is 24-bit, so the repeat count can cover the 16 MiB address space.

### 13.2 memcpy lowering

Source:

```text
memcpy(dst, src, len)
```

Lowering:

```text
ld hl, src
ld de, dst
ld bc, len
ldir
```

Rules:

```text
- use LDIR for non-overlapping forward copy
- use LDDR for backward copy when overlap requires it
- use byte stores for very small constant sizes if cheaper
- preserve volatile semantics
```

### 13.3 Small constant copy

For very small copies, inline stores may beat setup cost.

Suggested rules:

```text
size 0: remove
size 1: load/store byte
size 2: load/store u16 if aligned/valid, else two bytes
size 3: load/store u24 if valid, else three bytes
size 4..7: unrolled byte copy
size >= 8: LDIR
```

Tune thresholds using emulator cycle counts.

### 13.4 Volatile copy

Do not replace volatile copy loops with `LDIR` unless the semantics match exactly.

If source or destination is volatile:

```text
- keep explicit ordered accesses
- do not combine loads/stores
- do not use LDIR unless the platform declares that volatile block transfer is allowed
```

### 13.5 I/O block transfer

Use `INIRX`/`OTIRX` only for explicit port-stream APIs, not for ordinary memory copy.

Examples:

```text
io_read_stream(port_addr, dst, len)  -> INIRX
io_write_stream(port_addr, src, len) -> OTIRX
```

Rules:

```text
- DE holds stationary I/O address
- HL holds memory address
- BC holds length/count
- only use when target device contract supports repeated stationary I/O access
```

Do not use `INIRX`/`OTIRX` for framebuffer or asset memory copies.

---

## 14. memset / Fill Lowering

Recognize fills:

```text
memset(dst, value, len)
```

For constant zero and small sizes:

```text
- inline stores
- use XOR A for zero
```

For larger fills:

```text
- call __ezra_memset
```

A fast memset helper may use a seed byte plus block-copy trick if safe:

```text
(dst[0] = value)
LDIR from dst to dst + 1 for len - 1
```

Rules:

```text
- only for len > 0
- only for normal memory
- not for volatile memory unless explicitly allowed
```

---

## 15. Branch Layout and Pipeline-Aware Control Flow

The eZ80 pipeline must flush on control transfer such as `JP`, `CALL`, `RET`, `RST`, and similar instructions. Therefore, prefer fallthrough for the common path.

### 15.1 Branch likelihood sources

Branch likelihood may come from:

```text
- programmer hint
- loop backedge
- if condition pattern
- test/fail patterns
```

Syntax for hints:

```text
if likely(condition) {
    ...
}

if unlikely(error) {
    ...
}
```

### 15.2 Layout rule

For:

```text
if likely(cond) {
    hot()
} else {
    cold()
}
```

Prefer assembly shape:

```text
test cond
jr z, .cold
.hot:
  ...
  jr .end
.cold:
  ...
.end:
```

For:

```text
if unlikely(cond) {
    cold()
}
hot()
```

Prefer:

```text
test cond
jr nz, .cold
.hot:
  ...
  jr .end
.cold:
  ...
.end:
```

But if the cold block is tiny, choose code size over branch-likelihood.

### 15.3 Test/fail pattern

This source:

```text
if condition {
    test.pass()
} else {
    test.fail(1)
}
```

should prefer fallthrough to pass when test metadata says condition is expected true.

For normal game code, do not overfit. Use simple layout.

### 15.4 Loops

For loops:

```text
while cond {
    body
}
```

Use shape:

```text
jr .test
.body:
  ...
.test:
  test cond
  jr nz, .body
```

This has one branch per iteration and keeps body contiguous.

For `loop { ... }`, emit:

```text
.body:
  ...
  jr .body
```

---

## 16. Stack Traffic Reduction

Stack operations are more expensive in ADL because multibyte pushes/pops operate on 24-bit values.

Rules:

```text
- avoid push/pop in inner loops
- avoid spilling u8 values as 24-bit slots
- use fixed-size stack slots matching type width
- prefer caller-clobbered registers for short-lived temporaries
- avoid saving IX/IY unless function needs a frame pointer or index registers
```

### 16.1 Leaf function optimization

A leaf function that:

```text
- does not call other functions
- does not need stack locals
- does not use IX/IY
```

may omit a frame pointer.

Example:

```text
fn add(a: u24, b: u24) -> u24 {
    return a + b
}
```

should not create a stack frame.

### 16.2 Tiny function calling convention

Inline tiny functions when possible.

If not inlined, prefer register arguments according to the EZRA ABI.

Avoid stack argument passing for hot SDK functions.

### 16.3 Spill width

Spill slots use actual type width:

```text
u8/bool  -> 1 byte
u16/i16  -> 2 bytes
u24/i24  -> 3 bytes
ptr<T>   -> 3 bytes
```

Do not spill everything as 24-bit.

---

## 17. Register Selection

Default register roles:

```text
A       primary u8 accumulator
HL      primary u16/u24/ptr result
DE      secondary u16/u24/ptr operand
BC      count, tertiary operand, block length
IX      frame pointer when needed
IY      reserved for runtime or long-lived base pointer unless explicitly enabled
```

Rules:

```text
- prefer HL for pointer dereference
- prefer DE for destination pointer in LDIR
- prefer BC for lengths/counts
- avoid IX/IY in hot generated code unless they remove more expensive address recomputation
- avoid index-register addressing for simple locals when direct/HL addressing is cheaper
```

### 17.1 Pointer loops

For array traversal:

```text
while i != len {
    *(dst + i) = *(src + i)
    i += 1
}
```

Prefer pointer increments over repeated index scaling:

```text
HL = src
DE = dst
BC = len
LDIR
```

or for custom loops:

```text
HL = src
DE = dst
.loop:
  ld a, (hl)
  ld (de), a
  inc hl
  inc de
  dec bc
  ...
```

---

## 18. eZ80-Specific Instruction Selection

### 18.1 Zeroing A

Use:

```text
xor a
```

instead of:

```text
ld a, 0
```

when flags clobbering is allowed.

Rule:

```text
Use XOR A for zero only if flags are dead.
```

If flags must be preserved:

```text
ld a, 0
```

is required.

### 18.2 Testing A for zero

Use:

```text
or a
```

instead of:

```text
cp 0
```

when testing whether `A == 0`.

Rule:

```text
Use OR A only if changing flags according to OR semantics is acceptable.
```

### 18.3 Compare against zero for HL/u24

For multi-byte zero tests, prefer generated helper patterns.

Example for u24 in HL:

```text
; conceptual
ld a, h
or l
or hlu   ; assembler-specific upper-byte access may not exist directly
```

Because upper bytes of ADL registers may not be individually addressable in the same way, exact implementation must be verified against assembler/emulator support.

Fallback:

```text
store HL to temp
load three bytes
or them
```

Then peephole later.

### 18.4 Increment/decrement

Prefer `inc`/`dec` for ±1.

Examples:

```text
x = x + 1 -> inc
x = x - 1 -> dec
```

Rules:

```text
- only if flags effects are acceptable
- only if type width matches generated instruction
```

### 18.5 LEA

Use `LEA` for address arithmetic when it avoids multiple arithmetic instructions.

Example:

```text
p + small_signed_offset
```

may become:

```text
lea hl, ix+d
```

Rules:

```text
- use for frame/local access if IX/IY frame pointer is active
- use only if displacement fits
- avoid IX/IY if prefix/LEA cost exceeds benefit
```

---

## 19. ADL Suffix Rules

EZRA assembly generation should treat suffixes as semantic controls:

```text
Long data/address operation:
  use 24-bit registers/addresses

Short data/address operation:
  use 16-bit registers/addresses
```

Assembler spellings may be:

```text
.L
.S
.LIL
.SIS
.LIS
.SIL
```

depending on instruction and assembler.

### 19.1 Safe default

In ADL code, emit normal ADL instructions unless a short operation is proven safe.

### 19.2 Allowed short operation cases

Use short-width suffixes only for:

```text
- u16/i16 arithmetic where upper byte is dead
- u16/i16 comparisons where upper byte is dead
- u16 loop counters that never become addresses
- explicitly annotated short operations in inline asm/runtime
```

### 19.3 Forbidden short operation cases

Do not use short suffixes for:

```text
- pointer arithmetic
- memory addressing through HL/DE/BC
- stack pointer operations
- function call/return mechanics
- values live across calls as u24/i24/ptr
- globals or assets above 0x00FFFF
```

### 19.4 Explicit programmer override

EZRA may expose an unsafe intrinsic:

```text
unsafe_short16(expr)
```

or:

```text
asm volatile { "add.s hl, bc" }
```

But the compiler must not infer unsafe short operations unless its proof is simple.

---

## 20. Port and Volatile Barriers

### 20.1 Port barriers

These operations are ordered relative to each other:

```text
in PORT
out PORT, value
asm clobber ports
asm volatile
```

Do not reorder:

```text
out EXT_ADDR0, a0
out EXT_ADDR1, a1
out EXT_ADDR2, a2
out AUDIO_CMD, AUDIO_SUBMIT
```

### 20.2 Memory barriers

These operations are ordered relative to volatile memory:

```text
volatile load
volatile store
asm clobber memory
asm volatile
```

### 20.3 Normal memory

Normal memory may be optimized around pure operations, but not across:

```text
- function call with unknown effects
- asm memory clobber
- volatile memory operation when aliasing is possible
```

---

## 21. Inline Assembly Optimization Rules

Inline asm participates in optimization only through declared operands and clobbers.

### 21.1 Raw asm

```text
asm {
    "..."
}
```

Rules:

```text
- treated as volatile
- assumes memory and ports clobber unless explicitly marked pure
- not removed
- not reordered with volatile operations
```

### 21.2 Operand asm

```text
asm volatile(
    in x: u8 as reg8,
    out y: u8 as reg8,
    clobber a
) {
    "..."
}
```

Rules:

```text
- inputs are live before asm
- outputs are defined after asm
- clobbers kill corresponding register values
- memory clobber invalidates normal memory knowledge
- ports clobber prevents port operation reordering
```

### 21.3 Pure asm

Optional advanced feature:

```text
asm pure(
    in x: u8,
    out y: u8,
    clobber a,
    clobber flags
) {
    "..."
}
```

Rules:

```text
- no memory access
- no port access
- no hidden global state
- may be removed if output unused
- may be reordered like a pure expression
```

Do not implement `asm pure` until tests are strong.

---

## 22. Loop Recognition

Recognize common loops and replace them with runtime/block operations.

### 22.1 memcpy loop

Pattern:

```text
while i != len {
    *(dst + i) = *(src + i)
    i += 1
}
```

Lower to:

```text
LDIR
```

if:

```text
- source and destination are normal memory
- element type is u8
- no volatile access
- no alias hazard requiring backward copy
```

### 22.2 memset loop

Pattern:

```text
while i != len {
    *(dst + i) = value
    i += 1
}
```

Lower to:

```text
__ezra_memset
```

or inline for small constant sizes.

### 22.3 vram fill

If destination region is marked volatile `vram`, do not replace with normal `LDIR` unless the layout marks the region as:

```text
volatile block_transfer_ok
```

Add optional layout flag:

```text
region vram 0x080000..0x0BFFFF read write volatile block_transfer_ok;
```

Without this flag, keep explicit stores.

---

## 23. Strength Reduction

Replace expensive operations with cheaper ones.

Rules:

```text
x * 0 -> 0
x * 1 -> x
x * 2 -> x << 1
x * 4 -> x << 2
x * 8 -> x << 3

x / 1 -> x
unsigned x / 2 -> x >> 1
unsigned x / 4 -> x >> 2
unsigned x % 2 -> x & 1
unsigned x % 4 -> x & 3
```

Do not apply signed division/modulo transforms unless tested and exactly equivalent under EZRA wrapping semantics.

---

## 24. Common Subexpression Elimination

CSE is allowed only for pure expressions.

Allowed:

```text
let a = x + y
let b = x + y
```

becomes:

```text
let a = x + y
let b = a
```

Forbidden:

```text
let a = in PAD1_LO
let b = in PAD1_LO
```

Forbidden:

```text
let a = *FRAMEBUFFER
let b = *FRAMEBUFFER
```

if `FRAMEBUFFER` is volatile.

Memory loads from normal memory can be CSE’d only when no possible write/alias happens between them.

---

## 25. Peephole Optimization Rules

Run peephole after assembly emission.

### 25.1 Remove redundant load

```text
ld a, a
```

remove.

### 25.2 Zero A

```text
ld a, 0
```

becomes:

```text
xor a
```

only if flags are dead.

### 25.3 Test A

```text
cp 0
```

becomes:

```text
or a
```

only when A is already the tested value and flags effects are acceptable.

### 25.4 Redundant store/load

```text
ld (_x), a
ld a, (_x)
```

may become:

```text
ld (_x), a
```

only if:

```text
- _x is normal memory
- no volatile/asm/call between
- A is still live as the loaded value
```

### 25.5 Jump to next instruction

```text
jr .next
.next:
```

remove.

### 25.6 Invert branch over jump

```text
jr z, .true
jr .end
.true:
```

may become:

```text
jr nz, .end
```

if `.true` immediately follows and labels permit.

### 25.7 Do not peephole across labels

Never combine instructions across:

```text
- public label
- branch target
- asm block boundary
- volatile boundary
- debugger/source map boundary unless safe
```

---

## 26. Cost Model

Start with a simple cost model:

```text
cost = cycles + code_size_weight * bytes
```

Default:

```text
cycles weight = 1
code size weight = 0.25
```

Optimization modes:

```text
-O0   no optimization except required lowering
-O1   safe high-level simplification
-O2   normal game optimization
-Os   prefer smaller code
-Oz   aggressively smaller code
```

Recommended behavior:

```text
-O0:
  readable assembly, no inlining except required builtins

-O1:
  constant folding, dead branch elimination, simple DCE

-O2:
  inlining, width narrowing, MLT lowering, block copy recognition, branch layout, peephole

-Os:
  avoid inlining unless it reduces size
  prefer calls to helpers over unrolled code

-Oz:
  maximum helper calls
  minimum unrolling
```

---

## 27. Required Optimization Tests

### 27.1 MLT tests

```text
u8 * u8 -> u16
u16 * u16 wrapping
u24 * u24 wrapping
multiply by 0
multiply by 1
multiply by 2
signed multiply negative
```

### 27.2 ADL width tests

```text
u16 add does not corrupt later u24 value
ptr above 0x00FFFF still works
short-width optimization not applied to pointer
stack calls still return correctly
```

### 27.3 Port tests

```text
two port reads remain two reads
port write ordering preserved
audio_submit writes addr bytes before command
test.pass never optimized away
```

### 27.4 Volatile memory tests

```text
two volatile reads remain two reads
volatile store not removed
normal memory CSE works
volatile memory CSE does not
```

### 27.5 Block copy tests

```text
memcpy small size
memcpy large size
memcpy crossing 64 KiB boundary
memcpy overlapping forward
memcpy overlapping backward
vram copy respects layout flags
```

### 27.6 Branch tests

```text
likely branch preserves behavior
unlikely branch preserves behavior
while loop executes correct count
break/continue still target correct labels
```

### 27.7 Inline asm tests

```text
asm volatile not removed
memory clobber prevents stale load
ports clobber prevents port reordering
clobbered register is not reused
naked function emits no prologue
```

---

## 28. Implementation Order

Implement in this order:

```text
1. constant folding
2. dead branch elimination
3. dead code elimination for pure temporaries
4. inline explicitly marked tiny functions
5. width-aware instruction selection
6. MLT lowering for u8*u8
7. strength reduction
8. peephole zero/test/jump cleanup
9. block-copy lowering to LDIR
10. branch layout using likely/unlikely
11. stack traffic reduction
12. u16/u24 MLT-based runtime multiply helpers
```

Do not implement broad CSE until volatile/alias rules are solid.

Do not implement aggressive `.S`/short-mode suffix lowering until emulator tests specifically verify upper-byte behavior.

---

## 29. Core Optimization Rule

Every optimization must answer these questions before it is allowed:

```text
1. Does it preserve EZRA type-width wrapping?
2. Does it preserve signed/unsigned behavior?
3. Does it preserve port I/O order?
4. Does it preserve volatile memory order?
5. Does it respect inline asm clobbers?
6. Does it work above 0x00FFFF in ADL mode?
7. Does it pass emulator tests?
```

If any answer is uncertain, do not apply the optimization.
