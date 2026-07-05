
# EZRA Language, Runtime, and Cartridge Specification

## 1. Purpose

**EZRA** is a small compiled language for eZ80 ADL-mode game cartridges.

It is designed for:

```text
- eZ80 ADL fantasy-console games
- 24-bit memory
- explicit integer sizes
- direct port I/O
- direct memory access
- embedded binary assets
- inline assembly
- readable generated assembly
- emulator-based unit testing
```

EZRA is not C-compatible. It is intentionally smaller and more explicit.

Recommended tool names:

```text
Language:       EZRA
Source files:   .ezra
Compiler:       ezrac
Layout files:   .ezralayout
Runtime prefix: __ezra_*
SDK namespace:  ezra.*
Cart magic:     "EZRA"
```

---

## 2. CPU Model

EZRA enhanced cartridges run in **eZ80 ADL mode**.

Assumptions:

```text
- 24-bit address space
- 24-bit PC
- 24-bit SP
- 24-bit HL / DE / BC / IX / IY where ADL supports them
- 8-bit accumulator A
- 8-bit I/O ports
- little-endian memory layout
- interrupts disabled at startup unless explicitly enabled
```

EZRA does not target classic Z80 mode. Classic Z80 carts should use a separate target/toolchain.

---

## 3. Cartridge Kinds

The broader fantasy platform may support:

```text
Classic Z80 cart:
  - 64 KiB Z80-mode program
  - z88dk / assembly / SDCC-style workflow

EZRA eZ80 cart:
  - eZ80 ADL-mode program
  - 24-bit address space
  - EZRA compiler/runtime
```

This document defines the **EZRA eZ80 cartridge**.

---

## 4. Address Space

The eZ80 address space is:

```text
0x000000 - 0xFFFFFF
```

Total addressable memory:

```text
16 MiB
```

Default EZRA memory map:

```text
0x000000 - 0x00FFFF   low compatibility window / trap vectors / reserved
0x010000 - 0x01FFFF   cartridge header + startup + code
0x020000 - 0x03FFFF   read-only data / constants / tables
0x040000 - 0x07FFFF   general RAM: globals, bss, heap, scratch
0x080000 - 0x0BFFFF   video memory
0x0C0000 - 0x0FFFFF   audio memory
0x100000 - 0xDFFFFF   embedded cartridge assets
0xE00000 - 0xEFFFFF   runtime scratch / decompression / streaming
0xF00000 - 0xFFFFFF   stack and test-runner reserved space
```

Default symbols:

```text
EZRA_LOAD_ADDR      = 0x010000
EZRA_ENTRY_ADDR     = 0x010040
EZRA_CODE_BASE      = 0x010040
EZRA_RODATA_BASE    = 0x020000
EZRA_RAM_BASE       = 0x040000
EZRA_VRAM_BASE      = 0x080000
EZRA_AUDIO_BASE     = 0x0C0000
EZRA_ASSET_BASE     = 0x100000
EZRA_STACK_TOP      = 0xF00000
```

The stack grows downward.

---

## 5. Cartridge Header

Every EZRA cartridge begins with a header at `EZRA_LOAD_ADDR`.

Header layout:

```text
offset size  field
0x00   4     magic: "EZRA"
0x04   1     format_version
0x05   1     cpu_mode: 1 = eZ80 ADL
0x06   1     flags
0x07   1     reserved
0x08   3     entry_addr
0x0B   3     stack_top
0x0E   3     ram_base
0x11   3     vram_base
0x14   3     audio_base
0x17   3     asset_base
0x1A   2     header_size
0x1C   2     reserved
0x1E   3     layout_table_addr
0x21   3     asset_table_addr
0x24   3     symbol_table_addr
0x27   1     reserved
0x28   24    reserved for future fixed header fields
```

Default values:

```text
magic             = "EZRA"
format_version    = 1
cpu_mode          = 1
entry_addr        = 0x010040
stack_top         = 0xF00000
ram_base          = 0x040000
vram_base         = 0x080000
audio_base        = 0x0C0000
asset_base        = 0x100000
header_size       = 0x0040
```

All nonzero header pointer fields are absolute 24-bit eZ80 addresses, not file-relative offsets. A cart packer or loader can translate them to an image offset by subtracting `EZRA_LOAD_ADDR` for data stored directly in the cartridge image.

Program code starts at:

```text
0x010040
```

The current scaffold emits `layout_table_addr` and `symbol_table_addr` as newline-delimited ASCII tables for inspection and tests. Their binary encodings are not frozen yet.

Example symbol table:

```text
symbol __ezra_start 0x010040
symbol _main 0x010123
```

---

## 6. Layout Definition Format

EZRA uses a simple external memory layout file.

File extension:

```text
.ezralayout
```

The layout file tells the compiler/cart packer where sections, assets, RAM, stack, video memory, and audio memory live.

### 6.1 Default layout file

```text
layout ezra_default {
    load  0x010000;
    entry 0x010040;
    stack 0xF00000;

    region low       0x000000..0x00FFFF reserved;
    region code      0x010000..0x01FFFF read execute;
    region rodata    0x020000..0x03FFFF read;
    region ram       0x040000..0x07FFFF read write;
    region vram      0x080000..0x0BFFFF read write volatile;
    region audio     0x0C0000..0x0FFFFF read write volatile;
    region assets    0x100000..0xDFFFFF read;
    region scratch   0xE00000..0xEFFFFF read write;
    region stack     0xF00000..0xFFFFFF read write reserved;

    section .header  -> code   align 64;
    section .text    -> code   align 16;
    section .rodata  -> rodata align 16;
    section .data    -> ram    align 16;
    section .bss     -> ram    align 16;
    section .assets  -> assets align 256;
    section .scratch -> scratch align 16;

    symbol EZRA_LOAD_ADDR   = 0x010000;
    symbol EZRA_ENTRY_ADDR  = 0x010040;
    symbol EZRA_STACK_TOP   = 0xF00000;
    symbol EZRA_RAM_BASE    = 0x040000;
    symbol EZRA_VRAM_BASE   = 0x080000;
    symbol EZRA_AUDIO_BASE  = 0x0C0000;
    symbol EZRA_ASSET_BASE  = 0x100000;
}
```

### 6.2 Layout grammar

```text
layout NAME {
    load HEXADDR;
    entry HEXADDR;
    stack HEXADDR;

    region NAME START..END FLAGS*;
    section NAME -> REGION align INTEGER;
    symbol NAME = EXPR;
}
```

Region flags:

```text
read
write
execute
volatile
reserved
```

Rules:

```text
- Addresses are inclusive ranges.
- Regions may not overlap.
- Sections must map to declared regions.
- Section placement must fit inside the target region.
- `reserved` regions may not receive compiler-generated sections.
- `volatile` regions are treated as hardware-visible memory.
- `stack` defines the initial SP value.
- `entry` defines the startup address after the cartridge header.
```

### 6.3 Section semantics

Required sections:

```text
.header   cartridge header
.text     code
.rodata   constants, string literals, read-only tables
.data     initialized globals
.bss      zero-initialized globals
.assets   embedded byte assets
.scratch  runtime temporary storage
```

The compiler/cart packer must emit a map file showing final placement:

```text
section   start      end        size
.header   0x010000   0x01003F   0x000040
.text     0x010040   ...
.rodata   0x020000   ...
.assets   0x100000   ...
```

### 6.4 Custom layout example

A game can provide a larger asset area and smaller RAM area:

```text
layout big_asset_cart {
    load  0x010000;
    entry 0x010040;
    stack 0xF80000;

    region code      0x010000..0x02FFFF read execute;
    region ram       0x030000..0x04FFFF read write;
    region vram      0x080000..0x0BFFFF read write volatile;
    region audio     0x0C0000..0x0FFFFF read write volatile;
    region assets    0x100000..0xEFFFFF read;
    region stack     0xF80000..0xFFFFFF read write reserved;

    section .header -> code   align 64;
    section .text   -> code   align 16;
    section .rodata -> code   align 16;
    section .data   -> ram    align 16;
    section .bss    -> ram    align 16;
    section .assets -> assets align 256;

    symbol EZRA_VRAM_BASE  = 0x080000;
    symbol EZRA_AUDIO_BASE = 0x0C0000;
}
```

---

## 7. Example Default Port Map

All ports are 8-bit I/O ports. The following table is the default fantasy-console example map used by the scaffold and tests; other targets and SDKs may declare different port names and numbers with normal `port` declarations.

```text
0x01  IN    Controller 1 low byte
0x02  IN    Controller 1 high byte
0x03  IN    Controller 2 low byte
0x04  IN    Controller 2 high byte
0x05  IN    Controller 3 low byte
0x06  IN    Controller 3 high byte
0x07  IN    Controller 4 low byte
0x08  IN    Controller 4 high byte

0x09  OUT   Video command
0x0A  OUT   Audio command
0x0B  IN    System status
0x0C  OUT   Debug character

0x0D  OUT   Test result code
0x0E  OUT   Test halt command

0x10  OUT   Extended address byte 0, low
0x11  OUT   Extended address byte 1
0x12  OUT   Extended address byte 2, high
0x13  OUT   Extended length byte 0
0x14  OUT   Extended length byte 1
0x15  OUT   Extended mode / bank / flags
0x16  OUT   Extended command
0x17  IN    Extended status
```

Port accesses are always volatile.

The compiler must never delete, merge, or reorder port operations across other volatile operations.

---

## 8. Controller Layout

Each controller uses two bytes.

Low byte:

```text
bit 0 = B
bit 1 = Y
bit 2 = Select
bit 3 = Start
bit 4 = Up
bit 5 = Down
bit 6 = Left
bit 7 = Right
```

High byte:

```text
bit 0 = A
bit 1 = X
bit 2 = L
bit 3 = R
bit 4 = unused
bit 5 = unused
bit 6 = unused
bit 7 = unused
```

Button bits are active-high:

```text
0 = not pressed
1 = pressed
```

---

## 9. Source File Shape

EZRA source files use `.ezra`.

Example:

```text
import ezra.input
import ezra.video
import ezra.test

const START_X: i24 = 20 * 256

global player_x: i24 = START_X
global player_y: i24 = START_X

fn main() {
    let pad: u16 = input.read_pad(0)

    if input.pressed(pad, BTN_A) {
        test.pass()
    } else {
        test.fail(1)
    }
}
```

---

## 10. Modules

Modules are file-based.

```text
import ezra.input
import ezra.video
import ezra.audio
```

Rules:

```text
- one module per file
- declarations are private by default
- public declarations use `pub`
- cyclic imports are not allowed
```

Example:

```text
pub fn present() {
    out VIDEO_CMD, VIDEO_PRESENT
}
```

---

## 11. Primitive Types

Supported primitive types:

```text
u8     unsigned 8-bit integer
i8     signed 8-bit integer
u16    unsigned 16-bit integer
i16    signed 16-bit integer
u24    unsigned 24-bit integer
i24    signed 24-bit integer
bool   boolean
ptr24  raw 24-bit address
```

Typed pointers:

```text
ptr<u8>
ptr<u16>
ptr<u24>
ptr<Entity>
```

Unsupported:

```text
u32
i32
u64
i64
float
double
```

`u32` and `i32` are intentionally not part of the language.

Large math is done with explicit helper functions or assembly routines.

---

## 12. Integer Ranges

```text
u8:    0 to 255
i8:    -128 to 127

u16:   0 to 65,535
i16:   -32,768 to 32,767

u24:   0 to 16,777,215
i24:   -8,388,608 to 8,388,607
```

Unsigned arithmetic wraps modulo the type width.

Signed arithmetic uses two’s-complement representation and wraps on overflow.

EZRA arithmetic is fully defined. It does not have undefined signed overflow.

Unsigned division is ordinary integer division. Signed division truncates toward zero, so `-3 / 2 == -1`. Remainder uses the matching truncating-division rule and has the same sign as the dividend.

Division or remainder by zero evaluates to zero.

Right shift is logical for unsigned integers and arithmetic/sign-extending for signed integers.

---

## 13. Literals

Integer literals:

```text
123
0x7B
0b01111011
```

Typed suffixes:

```text
123u8
123i8
123u16
123i16
123u24
123i24
```

Character literals:

```text
'A'
'\n'
'\0'
```

Boolean literals:

```text
true
false
```

String literals are static zero-terminated byte arrays:

```text
"HELLO"
```

A string literal has type:

```text
ptr<u8>
```

---

## 14. Constants

Constants are compile-time evaluated.

```text
const BTN_A: u16 = 0x0100
const SUBPX_SHIFT: u8 = 8
const SUBPX_ONE: i24 = 256
const SCREEN_W: u16 = 320
const SCREEN_H: u16 = 240
```

Constant expressions support:

```text
+
-
*
/
%
&
|
^
~
<<
>>
==
!=
<
<=
>
>=
parentheses
casts
```

Constant evaluation uses the same arithmetic rules as runtime evaluation. Constant division or remainder by zero evaluates to zero.

---

## 15. Type Aliases

Type aliases are supported.

```text
alias pos = i24
alias vel = i24
alias tile_id = u16
alias entity_id = u8
```

Aliases do not create distinct types. They are naming conveniences.

Scaled-integer game math should use aliases and constants:

```text
alias subpx = i24

const SUBPX_SHIFT: u8 = 8
const SUBPX_ONE: subpx = 256
```

---

## 16. Embedded Bytes

EZRA has a built-in embedded byte asset feature.

Embedded byte declarations place data into a named output section, usually `.assets` or `.rodata`.

### 16.1 File embedding

```text
embed player_sprite: bytes = file("assets/player_sprite.bin") section .assets align 256
embed level_1: bytes = file("assets/level_1.map") section .assets align 256
embed song_1: bytes = file("assets/song_1.raw") section .assets align 256
```

Each embedded byte object exposes:

```text
player_sprite.ptr   -> ptr<u8>
player_sprite.len   -> u24
player_sprite.end   -> ptr<u8>
```

Example:

```text
video.blit(cast<ptr<u8>>(VRAM_BASE), player_sprite.ptr, player_sprite.len)
audio.submit(song_1.ptr, cast<u16>(song_1.len))
```

### 16.2 Inline byte embedding

```text
embed palette: bytes = bytes [
    0x00, 0x11, 0x22, 0x33,
    0x44, 0x55, 0x66, 0x77,
    0x88, 0x99, 0xAA, 0xBB,
    0xCC, 0xDD, 0xEE, 0xFF
] section .rodata align 16
```

### 16.3 Text embedding

```text
embed title_text: bytes = text("EZRA DEMO") section .rodata align 1
embed title_cstr: bytes = cstr("EZRA DEMO") section .rodata align 1
```

Rules:

```text
text("...") emits raw bytes without a trailing zero.
cstr("...") emits bytes with a trailing zero.
```

### 16.4 Repeated byte data

```text
embed empty_tile: bytes = repeat(0x00, 64) section .assets align 16
embed solid_tile: bytes = repeat(0xFF, 64) section .assets align 16
```

### 16.5 Typed byte views

Raw embedded bytes are byte-addressed. The compiler does not assume alignment beyond the declared `align`.

A typed view can be requested explicitly:

```text
let p: ptr<u8> = player_sprite.ptr
let first: u8 = *p
```

No automatic struct or array deserialization is performed.

### 16.6 Embedded byte rules

```text
- embedded objects are read-only by default
- embedded object names are global symbols
- `.ptr`, `.len`, and `.end` are compiler-generated properties
- embedded file paths are relative to the source file or project root
- embedded assets must fit inside their target section/region
- alignment must be a power of two
- duplicate embedded names are compile errors
```

### 16.7 Asset table

The cart packer emits an asset table if any `embed` declarations exist.

Asset table entry:

```text
offset size field
0x00   3    asset_addr
0x03   3    asset_len
0x06   2    name_offset
0x08   1    section_id
0x09   1    flags
```

The asset table is optional for runtime use but useful for debugging, inspection, and tooling.

---

## 17. Variables

Global variables:

```text
global player_x: i24 = 0
global player_y: i24 = 0
global score: u24 = 0
```

Local variables:

```text
let x: i24 = 0
let pad: u8 = in PAD1_LO
```

Rules:

```text
- globals live in RAM
- locals live in registers, stack slots, or compiler-generated temporaries
- every variable declaration requires a type
- shadowing is not allowed
```

---

## 18. Arrays

Static arrays are supported.

```text
global palette: [u8; 16] = [
    0, 1, 2, 3,
    4, 5, 6, 7,
    8, 9, 10, 11,
    12, 13, 14, 15
]
```

Array indexing:

```text
let x: u8 = palette[3]
palette[3] = 12
```

Rules:

```text
- index type must be u8, u16, or u24
- runtime bounds checks are not generated
- compile-time known out-of-bounds indexes are compile errors
- array elements are stored compactly
- array pointer decay does not exist
```

Use explicit address-of:

```text
let p: ptr<u8> = &palette[0]
```

---

## 19. Structs

Structs are supported.

```text
struct Entity {
    x: i24
    y: i24
    vx: i24
    vy: i24
    sprite: u8
    flags: u8
}
```

Struct layout:

```text
- fields are stored in declaration order
- no implicit padding
- alignment is 1 byte
- u16 occupies 2 bytes
- u24 and ptr24 occupy 3 bytes
```

Example:

```text
global player: Entity = Entity {
    x: 0,
    y: 0,
    vx: 0,
    vy: 0,
    sprite: 1,
    flags: 0
}

player.x = player.x + player.vx
```

Structs are passed by pointer. Passing structs by value is not supported.

---

## 20. Pointers

Pointer type:

```text
ptr<T>
```

Raw address type:

```text
ptr24
```

Pointer operations:

```text
&x          address of variable
*p          dereference
p + n       pointer addition
p - n       pointer subtraction
```

Rules:

```text
- pointer values are 24-bit
- pointer arithmetic scales by the pointed-to type size
- dereferencing ptr<T> loads or stores T
- null pointer is 0x000000
```

Examples:

```text
let p: ptr<u8> = &palette[0]
let first: u8 = *p
*(p + 1) = 7
```

---

## 21. Volatile Memory

Volatile memory declarations are supported.

```text
volatile mmio FRAMEBUFFER: ptr<u8> = 0x080000
```

Volatile rules:

```text
- volatile loads are never removed
- volatile stores are never removed
- volatile operations are not reordered across other volatile operations
- volatile pointer dereferences emit real memory access
```

Example:

```text
*(FRAMEBUFFER + 0) = 12
let px: u8 = *(FRAMEBUFFER + 0)
```

---

## 22. Ports

Ports are named hardware resources. The examples below use the default fantasy-console port map, but applications and SDKs may declare any target-specific 8-bit I/O ports needed by the hardware.

```text
port PAD1_LO: u8 = 0x01
port PAD1_HI: u8 = 0x02
port VIDEO_CMD: u8 = 0x09
port DEBUG_CHAR: u8 = 0x0C
port TEST_RESULT: u8 = 0x0D
port TEST_HALT: u8 = 0x0E
```

Read syntax:

```text
let pad: u8 = in PAD1_LO
```

Write syntax:

```text
out DEBUG_CHAR, 'A'
```

Rules:

```text
- ports are not pointers
- ports are not memory
- port type must be u8
- port access is volatile
```

---

## 23. Operators

Arithmetic:

```text
+
-
*
/
%
```

Bitwise:

```text
&
|
^
~
<<
>>
```

Comparison:

```text
==
!=
<
<=
>
>=
```

Logical:

```text
&&
||
!
```

Assignment:

```text
=
+=
-=
&=
|=
^=
<<=
>>=
```

Rules:

```text
- arithmetic operands must have same signedness and width, unless one side is a literal that fits
- widening requires explicit cast except literal widening
- narrowing requires explicit cast
- signed/unsigned mixing requires explicit cast
- bool is not an integer
```

Example:

```text
let a: u8 = 10
let b: u16 = cast<u16>(a)
let c: u8 = cast<u8>(b)
```

Runtime multiplication/division are supported by compiler-emitted runtime helper calls.

---

## 24. Casts

Cast syntax:

```text
cast<u8>(x)
cast<i24>(x)
cast<ptr<u8>>(addr)
```

Rules:

```text
- integer widening preserves value
- integer narrowing truncates high bits
- signed/unsigned casts preserve bit pattern
- integer-to-pointer casts require u24 or ptr24
- pointer-to-integer casts produce u24 or ptr24
```

Example:

```text
let addr: u24 = 0x080000
let fb: ptr<u8> = cast<ptr<u8>>(addr)
let raw: ptr24 = cast<ptr24>(fb)
```

---

## 25. Control Flow

If:

```text
if condition {
    ...
} else {
    ...
}
```

While:

```text
while condition {
    ...
}
```

Infinite loop:

```text
loop {
    ...
}
```

Break and continue:

```text
break
continue
```

Return:

```text
return
return value
```

Conditions must be `bool`.

Integer-to-bool conversion is explicit:

```text
if (pad & BTN_A) != 0 {
    ...
}
```

---

## 26. Functions

Function syntax:

```text
fn add(a: u24, b: u24) -> u24 {
    return a + b
}
```

Void function:

```text
fn present() {
    out VIDEO_CMD, 1
}
```

Rules:

```text
- functions are globally named
- recursion is allowed
- function overloading is not allowed
- varargs are not allowed
- structs are passed by pointer only
- arrays are passed by pointer only
```

Attributes:

```text
inline
naked
interrupt
pub
extern asm
```

Example:

```text
inline fn pressed(pad: u16, button: u16) -> bool {
    return (pad & button) != 0
}
```

---

## 27. Calling Convention

Internal EZRA calling convention:

Return values:

```text
bool/u8/i8      -> A
u16/i16         -> HL low 16 bits
u24/i24/ptr24   -> HL
```

Arguments:

```text
arg1 u8/i8/bool     -> A
arg1 u16/u24/ptr24  -> HL

arg2 u16/u24/ptr24  -> DE
arg3 u16/u24/ptr24  -> BC

additional args     -> stack, right to left
```

Caller-clobbered:

```text
AF
BC
DE
HL
```

Callee-preserved:

```text
IX
IY
SP
```

Rules:

```text
- function calls may clobber AF, BC, DE, HL
- inline asm must declare clobbers
- interrupt handlers use the interrupt convention
```

---

## 28. Stack Frames

IX is the frame pointer for functions that need stack locals or stack arguments.

Stack slot sizes:

```text
u8/bool   = 1 byte
u16/i16   = 2 bytes
u24/i24   = 3 bytes
ptr24     = 3 bytes
```

Stack grows downward.

The compiler must keep stack frames byte-accurate. ADL return addresses are 24-bit.

Function prologue concept:

```text
push ix
ld ix, 0
add ix, sp
; reserve local bytes
```

Function epilogue concept:

```text
; release local bytes
pop ix
ret
```

Exact final assembly syntax depends on the selected assembler and verified emulator behavior.

---

## 29. Inline Assembly

Inline assembly is part of EZRA.

Raw inline assembly:

```text
asm {
    "ld a, 1"
    "out0 (09h), a"
}
```

Volatile inline assembly:

```text
asm volatile {
    "di"
    "ei"
}
```

Operand inline assembly:

```text
asm volatile(
    in ch: u8,
    clobber a,
    clobber ports
) {
    "ld a, {ch}"
    "out0 (0Ch), a"
}
```

Returning a value:

```text
fn read_pad1_low_raw() -> u8 {
    let result: u8 = 0

    asm volatile(
        out result: u8 as reg8,
        clobber a,
        clobber ports
    ) {
        "in0 a, (01h)"
        "ld {result}, a"
    }

    return result
}
```

### 29.1 Inline asm forms

```text
asm { ... }
asm volatile { ... }
asm(...) { ... }
asm volatile(...) { ... }
```

### 29.2 Operand classes

```text
reg8      8-bit register-compatible value
reg16     16-bit register-compatible value
reg24     24-bit register-compatible value
mem       addressable stack/global slot
imm       compile-time immediate
```

Example:

```text
asm volatile(
    in addr: ptr<u8> as reg24,
    out value: u8 as reg8,
    clobber a,
    clobber hl,
    clobber memory
) {
    "ld hl, {addr}"
    "ld a, (hl)"
    "ld {value}, a"
}
```

### 29.3 Clobbers

Allowed clobbers:

```text
a
f
af
b
c
bc
d
e
de
h
l
hl
ix
iy
sp
memory
ports
flags
```

Special clobbers:

```text
memory   asm may read/write arbitrary memory
ports    asm may read/write I/O ports
flags    asm changes condition flags
```

Rules:

```text
- modifying a register without declaring it is invalid
- modifying SP is only allowed in naked functions
- modifying IX/IY requires clobber declaration
- asm volatile prevents removal
- clobber memory prevents memory reordering across asm
- clobber ports prevents port reordering across asm
```

### 29.4 Naked functions

Naked functions suppress compiler prologue/epilogue.

```text
naked fn raw_entry() {
    asm volatile {
        "ld sp, 0F00000h"
        "call _main"
        "jp $"
    }
}
```

Rules:

```text
- naked functions may contain only asm blocks
- naked functions may not use locals
- naked functions may not use normal return
- naked functions are responsible for preserving registers
```

### 29.5 Extern assembly functions

Assembly functions may be declared:

```text
extern asm fn memcpy_fast(dst: ptr<u8>, src: ptr<u8>, len: u24)
extern asm fn mul_u24(a: u24, b: u24) -> u24
```

They use the EZRA calling convention.

---

## 30. Interrupts

Interrupt functions are supported.

```text
interrupt fn vblank_irq() {
    ...
}
```

Rules:

```text
- interrupt functions use interrupt prologue/epilogue
- interrupt functions return with `reti`
- interrupt functions preserve all registers unless marked naked
- interrupt functions may call normal functions only if reentrancy is safe
```

Naked interrupt:

```text
naked interrupt fn raw_irq() {
    asm volatile {
        "push af"
        "push hl"
        "; handler body"
        "pop hl"
        "pop af"
        "reti"
    }
}
```

---

## 31. Video Runtime

Default symbols:

```text
const VRAM_BASE: ptr<u8> = 0x080000
const VIDEO_PRESENT: u8 = 1
const VIDEO_CLEAR: u8 = 2
const VIDEO_SET_MODE: u8 = 3

port VIDEO_CMD: u8 = 0x09
```

Required SDK functions:

```text
fn present()
fn clear(value: u8)
fn poke(offset: u24, value: u8)
fn peek(offset: u24) -> u8
fn blit(dst: ptr<u8>, src: ptr<u8>, len: u24)
```

Baseline semantics:

```text
present():
  writes VIDEO_PRESENT to VIDEO_CMD

poke(offset, value):
  writes value to VRAM_BASE + offset

peek(offset):
  reads VRAM_BASE + offset
```

The compiler does not hardcode a video mode. Video mode is SDK/runtime-defined.

---

## 32. Audio Runtime

Default symbols:

```text
const AUDIO_BASE: ptr<u8> = 0x0C0000
const AUDIO_SUBMIT_BUFFER: u8 = 1
const AUDIO_STOP: u8 = 2

port AUDIO_CMD: u8 = 0x0A
port EXT_ADDR0: u8 = 0x10
port EXT_ADDR1: u8 = 0x11
port EXT_ADDR2: u8 = 0x12
port EXT_LEN0: u8 = 0x13
port EXT_LEN1: u8 = 0x14
port EXT_COMMAND: u8 = 0x16
```

Required SDK functions:

```text
fn audio_submit(addr: ptr<u8>, len: u16)
fn audio_stop()
fn poke_audio(offset: u24, value: u8)
fn peek_audio(offset: u24) -> u8
```

`audio_submit(addr, len)` writes the 24-bit address to ports `0x10..0x12`, length to `0x13..0x14`, then writes `AUDIO_SUBMIT_BUFFER` to `AUDIO_CMD`.

---

## 33. Standard SDK Modules

Example SDK modules:

```text
ezra.core
ezra.input
ezra.video
ezra.audio
ezra.debug
ezra.mem
ezra.math
ezra.test
```

These modules are platform libraries built from normal EZRA features such as constants, `port` declarations, volatile MMIO declarations, functions, and inline assembly. They are not language intrinsics, and the compiler should not hardcode controller, video, or audio behavior into ordinary codegen.

Targets may provide different SDKs for hardware such as the TI-84 Plus CE or Agon Light. Those SDKs should follow the same rules: expose typed constants and functions over generic port/MMIO primitives, keep volatile operations visible in generated assembly, and use compiler intrinsics only for target-independent operations.

### 33.1 ezra.input Example

```text
pub const BTN_B: u16      = 0x0001
pub const BTN_Y: u16      = 0x0002
pub const BTN_SELECT: u16 = 0x0004
pub const BTN_START: u16  = 0x0008
pub const BTN_UP: u16     = 0x0010
pub const BTN_DOWN: u16   = 0x0020
pub const BTN_LEFT: u16   = 0x0040
pub const BTN_RIGHT: u16  = 0x0080
pub const BTN_A: u16      = 0x0100
pub const BTN_X: u16      = 0x0200
pub const BTN_L: u16      = 0x0400
pub const BTN_R: u16      = 0x0800

pub fn read_pad(index: u8) -> u16
pub fn pressed(pad: u16, button: u16) -> bool
```

### 33.2 ezra.video Example

```text
pub const VRAM_BASE: ptr<u8> = 0x080000

pub fn present()
pub fn clear(value: u8)
pub fn poke(offset: u24, value: u8)
pub fn peek(offset: u24) -> u8
pub fn blit(dst: ptr<u8>, src: ptr<u8>, len: u24)
```

### 33.3 ezra.audio Example

```text
pub const AUDIO_BASE: ptr<u8> = 0x0C0000

pub fn submit(addr: ptr<u8>, len: u16)
pub fn stop()
pub fn poke(offset: u24, value: u8)
```

### 33.4 ezra.debug

```text
pub fn char(ch: u8)
pub fn str(s: ptr<u8>)
pub fn hex_u8(v: u8)
pub fn hex_u16(v: u16)
pub fn hex_u24(v: u24)
```

### 33.5 ezra.mem

```text
pub fn memcpy(dst: ptr<u8>, src: ptr<u8>, len: u24)
pub fn memset(dst: ptr<u8>, value: u8, len: u24)
pub fn peek8(addr: ptr<u8>) -> u8
pub fn poke8(addr: ptr<u8>, value: u8)
```

### 33.6 ezra.math

No floating point.

Scaled-integer helpers:

```text
pub const SUBPX_SHIFT: u8 = 8
pub const SUBPX_ONE: i24 = 256

pub fn subpx_from_int(v: i16) -> i24
pub fn subpx_to_int(v: i24) -> i16
pub fn mul_i24(a: i24, b: i24) -> i24
pub fn div_i24(a: i24, b: i24) -> i24
pub fn sin_u8(angle: u8) -> i16
pub fn cos_u8(angle: u8) -> i16
```

### 33.7 ezra.test

```text
pub fn pass()
pub fn fail(code: u8)
pub fn assert_eq_u8(a: u8, b: u8, code: u8)
pub fn assert_eq_u16(a: u16, b: u16, code: u8)
pub fn assert_eq_u24(a: u24, b: u24, code: u8)
```

---

## 34. Program Entry

Every cartridge must define:

```text
fn main()
```

A normal game may define:

```text
fn init()
fn update()
fn draw()
```

Typical game entry:

```text
fn main() {
    init()

    loop {
        update()
        draw()
        video.present()
    }
}
```

---

## 35. Example Game

```text
import ezra.input
import ezra.video
import ezra.math

alias pos = i24

embed player_sprite: bytes = file("assets/player.bin") section .assets align 256

global player_x: pos = 20 * SUBPX_ONE
global player_y: pos = 20 * SUBPX_ONE

fn init() {
    video.clear(0)
}

fn update() {
    let pad: u16 = input.read_pad(0)

    if input.pressed(pad, BTN_LEFT) {
        player_x -= 1 * SUBPX_ONE
    }

    if input.pressed(pad, BTN_RIGHT) {
        player_x += 1 * SUBPX_ONE
    }

    if input.pressed(pad, BTN_UP) {
        player_y -= 1 * SUBPX_ONE
    }

    if input.pressed(pad, BTN_DOWN) {
        player_y += 1 * SUBPX_ONE
    }
}

fn draw() {
    let sx: u16 = cast<u16>(player_x >> SUBPX_SHIFT)
    let sy: u16 = cast<u16>(player_y >> SUBPX_SHIFT)
    let offset: u24 = cast<u24>(sy) * 320 + cast<u24>(sx)

    video.poke(offset, 15)
}

fn main() {
    init()

    loop {
        update()
        draw()
        video.present()
    }
}
```

---

## 36. Runtime Assembly Helpers

The runtime must provide:

```text
__ezra_start
__ezra_exit
__ezra_pass
__ezra_fail

__ezra_memcpy
__ezra_memset

__ezra_mul_u8
__ezra_mul_u16
__ezra_mul_u24
__ezra_mul_i24

__ezra_div_u8
__ezra_div_u16
__ezra_div_u24
__ezra_div_i24

__ezra_mod_u8
__ezra_mod_u16
__ezra_mod_u24
__ezra_mod_i24
```

Compiler-generated code may call these helpers.

Helpers use the EZRA calling convention unless declared otherwise.

---

## 37. Assembly Output Requirements

The compiler emits readable eZ80 ADL assembly.

Generated assembly should include source comments in debug mode.

Example:

```text
; source: player_x += player_vx
ld hl, (_player_x)
ld de, (_player_vx)
add hl, de
ld (_player_x), hl
```

Required output files:

```text
game.asm       generated assembly
game.map       section/symbol map
game.ezra.cart final cartridge image
```

Required sections:

```text
.header
.text
.rodata
.data
.bss
.assets
.scratch
```

---

## 38. Test Runner Contract

The test runner loads assembled code at:

```text
0x010000
```

Initial machine state:

```text
PC = 0x010000
SP = value from layout, default 0xF00000
ADL mode enabled
interrupts disabled
RAM initialized to 0 unless test overrides it
ports initialized to 0 unless test overrides them
```

A test passes when the program writes:

```text
OUT 0x0D, 0
OUT 0x0E, 1
```

A test fails when the program writes:

```text
OUT 0x0D, nonzero
OUT 0x0E, 1
```

A test also fails on:

```text
- emulator error
- illegal instruction
- timeout
- execution outside mapped memory
- stack overflow into non-stack memory
```

Default instruction budget:

```text
1,000,000 instructions
```

Runtime test example:

```text
import ezra.test

fn main() {
    let x: u24 = 0x010000
    let y: u24 = 0x000123
    let z: u24 = x + y

    test.assert_eq_u24(z, 0x010123, 1)
    test.pass()
}
```

Controller test example:

```text
import ezra.input
import ezra.test

fn main() {
    let pad: u16 = input.read_pad(0)

    if input.pressed(pad, BTN_UP) {
        test.pass()
    } else {
        test.fail(1)
    }
}
```

Test metadata:

```text
port 0x01 = 0x10
```

---

## 39. Compiler Pipeline

Required compiler pipeline:

```text
source
  -> pest parse tree
  -> AST
  -> name resolution
  -> type checking
  -> typed IR
  -> simple optimization
  -> eZ80 ADL assembly
  -> assembler
  -> cartridge packer
  -> emulator test runner
```

Required optimizations:

```text
- constant folding
- constant propagation
- dead code elimination
- inline functions marked `inline`
- remove unused private functions
- peephole cleanup for obvious redundant loads/stores
```

The optimizer must respect:

```text
- volatile memory
- port operations
- asm volatile
- memory clobbers
- port clobbers
```

---

## 40. Diagnostics

Compiler errors must include:

```text
file
line
column
message
```

Required compile-time errors:

```text
- unknown identifier
- duplicate declaration
- type mismatch
- narrowing without cast
- signed/unsigned mix without cast
- invalid port type
- invalid pointer dereference
- out-of-range literal
- array index out of bounds when known at compile time
- struct field does not exist
- missing return value
- inline asm output type mismatch
- inline asm undeclared clobber
- embedded file not found
- embedded asset exceeds target section/region
- layout region overlap
- section does not fit in region
```

---

## 41. Grammar Sketch

```text
program       = decl*

decl          = import_decl
              | const_decl
              | alias_decl
              | port_decl
              | mmio_decl
              | embed_decl
              | global_decl
              | struct_decl
              | extern_decl
              | fn_decl

import_decl   = "import" path

const_decl    = visibility? "const" ident ":" ty "=" expr
alias_decl    = visibility? "alias" ident "=" ty
port_decl     = visibility? "port" ident ":" ty "=" expr
mmio_decl     = visibility? "volatile"? "mmio" ident ":" ty "=" expr

embed_decl    = visibility? "embed" ident ":" "bytes" "=" embed_source embed_opts?
embed_source  = file_embed | bytes_embed | text_embed | cstr_embed | repeat_embed
file_embed    = "file" "(" string_lit ")"
bytes_embed   = "bytes" "[" byte_list? "]"
text_embed    = "text" "(" string_lit ")"
cstr_embed    = "cstr" "(" string_lit ")"
repeat_embed  = "repeat" "(" expr "," expr ")"
embed_opts    = ("section" section_name)? ("align" int_lit)?

global_decl   = visibility? "global" ident ":" ty "=" expr

struct_decl   = visibility? "struct" ident "{" field* "}"
field         = ident ":" ty

extern_decl   = "extern" "asm" "fn" ident "(" params? ")" ret_ty?

fn_decl       = attrs? visibility? "fn" ident "(" params? ")" ret_ty? block
attrs         = ("inline" | "naked" | "interrupt")*

params        = param ("," param)*
param         = ident ":" ty
ret_ty        = "->" ty

block         = "{" stmt* "}"

stmt          = let_stmt
              | assign_stmt
              | if_stmt
              | while_stmt
              | loop_stmt
              | break_stmt
              | continue_stmt
              | return_stmt
              | out_stmt
              | asm_stmt
              | expr_stmt

let_stmt      = "let" ident ":" ty "=" expr
assign_stmt   = place assign_op expr
if_stmt       = "if" expr block ("else" block)?
while_stmt    = "while" expr block
loop_stmt     = "loop" block
break_stmt    = "break"
continue_stmt = "continue"
return_stmt   = "return" expr?
out_stmt      = "out" ident "," expr

asm_stmt      = "asm" "volatile"? asm_operands? "{" asm_lines "}"

ty            = primitive_ty
              | "ptr" "<" ty ">"
              | "[" ty ";" expr "]"
              | ident

primitive_ty  = "u8" | "i8" | "u16" | "i16" | "u24" | "i24" | "bool" | "ptr24"

expr          = logical_or
```

---

## 42. Design Rules

EZRA follows these rules:

```text
- 24-bit is normal.
- u32 does not exist.
- ports are not memory.
- volatile means real hardware access.
- embedded bytes are first-class cartridge assets.
- memory layout is explicit and inspectable.
- inline assembly is allowed but must declare clobbers.
- struct layout is compact and predictable.
- game code should be readable.
- hot paths may use assembly helpers.
- compiler output must be testable in the emulator.
```
