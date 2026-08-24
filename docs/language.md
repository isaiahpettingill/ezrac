# EZRA Language Documentation

This document describes the EZRA source language implemented by `ezrac` today. `spec.md` is the broader design document; this file is intended as day-to-day language documentation for code that should parse and build with the current compiler.

## Source Files

EZRA source files use the `.ezra` extension. Files are UTF-8 text. Line comments start with `//` and continue to the end of the line.

Statements and declarations may end with `;`, but semicolons are generally optional because newlines and block boundaries are accepted by the parser.

```ezra
// hello.ezra
fn main() {
    let value: u8 = 42
}
```

## Program Entry

Executable programs must define `fn main()` with no parameters and no return type.

```ezra
fn main() {
    return
}
```

Imported files may define helper functions. Imported `main` functions are ignored so a library file can include local examples without replacing the root program entry point.

## Names And Paths

Identifiers start with an ASCII letter or `_` and continue with ASCII letters, digits, or `_`.

```text
name
_private
counter2
```

Paths join identifiers with dots and are used for imports, SDK-style names, type names, and calls.

```ezra
import agon.console

fn main() {
    console.print("hello")
    agon.console.newline()
}
```

When importing a public module item, `ezrac` creates aliases with the full import prefix, such as `agon.console.print`. If only one imported module has a given last component, it also creates a short alias, such as `console.print`.

## Visibility

Top-level declarations are private by default. Add `pub` to expose declarations to importers.

```ezra
pub const WIDTH: u16 = 320

pub fn draw() {
}
```

Private declarations are usable inside the defining file. Public declarations are re-exported as module aliases when imported.

## Imports

`import` loads another `.ezra` file or a built-in SDK module.

```ezra
import math
import agon.console
```

For `import foo.bar`, `ezrac` searches for `foo/bar.ezra` relative to the importing file, ancestor directories, the current working directory, configured SDK paths, and finally built-in target SDK modules.

Imports are resolved recursively. Cyclic imports are rejected. Duplicate imports are de-duplicated.

The compiler provides three built-in intrinsic catalogs. Import them like ordinary modules:

```ezra
import ezra.bits
import ezra.int
import ezra.mem
```

The full names (`ezra.bits.rotate_left`) and imported short names (`bits.rotate_left`) resolve to the same catalog entry. These are compiler intrinsics, not user-editable SDK source files. Their source type and constant checks run before target lowering; a target can still diagnose an operation or width it cannot represent.

## Conditional Compilation

Add `@cfg(...)` before any top-level declaration to include it only for matching targets or compiler mode.

```ezra
@cfg(cpu("ez80"))
pub const POINTER_BYTES: u8 = 3

@cfg(any(cpu("z80"), cpu("z180")))
pub const POINTER_BYTES: u8 = 2
```

Supported predicates:

```text
target("full-target-triple")
target_family("first-target-part")
cpu("ez80" | "z80" | "r800" | "z80n" | "z180" | "i8080" | "i8085" | "lr35902")
vendor("second-target-part")
os("mos" | "cpm" | "baremetal")
pointer_width(16 | 24)
address_width(16 | 24)
feature("target-part")
debug
release
all(...)
any(...)
not(...)
```

`feature("...")` matches a target-triple component other than the CPU. Unknown feature names are rejected rather than silently evaluating to false.

### Explicit Banking Syntax

`@cfg(bank(N))` is a distinct top-level bank-placement attribute, not a conditional-compilation predicate. The parser preserves it alongside any ordinary `@cfg(...)` conditions:

```ezra
@cfg(bank(3))
pub fn level_loader() {}
```

Pointer expressions may carry an explicit bank postfix. Parenthesize compound pointer expressions before the postfix:

```ezra
let tiles: ptr<u8> = tile_data@3
let next_tile: ptr<u8> = (tiles + 16)@3
```

Enable the project-level syntax/configuration foundation with:

```toml
[banking]
enabled = true
```

This currently records source and project metadata only. Bank switching, target eligibility, pointer representation, linking, and runtime behavior remain target-owned follow-up work.

## Types

Primitive integer types are explicit:

```text
u8   i8
u16  i16
u24  i24
u32  i32 (target-dependent)
```

Other built-in names used by the compiler and SDK include `bool` and `bytes`. Paths may also name structs or aliases.

Pointers use `ptr<T>` and arrays use `[T; LEN]`. Arrays are storage, not return values. Pass arrays through pointer parameters, normally as `ptr<[T; LEN]>`; use the same form for output arrays. Arrays do not decay to element pointers. To perform element-pointer arithmetic, explicitly cast the array address to `ptr<T>`.

```ezra
alias Byte = u8

global counter: u8 = 0
global buffer: [u8; 16] = [0, 0, 0, 0]
global framebuffer: ptr<u8> = 0x080000

fn clear(buffer: ptr<[u8; 16]>) {
    // Write the array through `buffer`.
}

fn main() {
    clear(&buffer)
    let bytes: ptr<u8> = cast<ptr<u8>>(&buffer)
    *(bytes + 1) = 0
}
```

The target controls pointer width. eZ80, WDC 65C816, and the generic M68k target use 24-bit pointers. MSP430X targets use 20-bit pointers; Z80-family, 8080/8085, LR35902, MOS 6502, base MSP430, TMS9900, and AVR targets use 16-bit pointers. Integer widths and pointer widths are separate: a target may accept a source type but reject an intrinsic combination that its ABI or emitter cannot lower.

## Literals

Integers may be decimal, hexadecimal, or binary. Add an integer suffix to force a specific integer type.

```ezra
42
0x2A
0b101010
42u8
0x1000u16
0x040045u24
```

Booleans are `true` and `false`.

Characters use single quotes and evaluate to one byte.

```ezra
'A'
'\n'
'\0'
```

Strings use double quotes. A string is immutable byte storage: it can be passed as a pointer and accessed as a byte array. Cast it to `ptr<u8>` before element-pointer arithmetic.

```ezra
"hello"
"line\n"
```

Supported escapes are `\n`, `\0`, `\t`, `\\`, `\'`, and `\"`.

## Constants, Globals, Ports, And MMIO

Constants are compile-time named values.

```ezra
const MAX_LIVES: u8 = 3
pub const SCREEN_BASE: u24 = 0x080000u24
```

Globals allocate mutable storage in the program data area.

```ezra
global score: u16 = 0
```

### Compile-Time Evaluation

Constant expressions are folded by default. This includes arithmetic, boolean and bitwise operations, casts, references to other enabled constants, and indexing an immutable constant array with a known index. Array initializers may be shorter than their declared length; missing elements are zero-filled for constant indexing. `zeroes()` is accepted for fixed-size array initializers and produces the same zero-filled storage.

```ezra
const OFFSET: u8 = 2 + 3
const TABLE: [u8; 3] = [4, 7, 9]
const ZEROES: [u8; 4] = zeroes()

fn read_table() -> u8 {
    return TABLE[1] + ZEROES[3] + OFFSET
}
```

Mark a pure function `@comptime` to evaluate calls when every input is known:

```ezra
@comptime fn add(left: u8, right: u8) -> u8 {
    return left + right
}

fn answer() -> u8 {
    return add(2, 3)
}
```

`@no-comptime` is exact and disables compile-time folding for a constant and comptime evaluation or inlining for a function. Mutable globals, ports, MMIO, pointers and strings are never evaluated at comptime. Functions with assignments, loops, recursion, inline assembly, or other effectful calls stay as runtime calls. If an input is unknown or an evaluation limit is reached, the compiler keeps the original expression and continues with normal code generation.

```ezra
@no-comptime const RUNTIME_VALUE: u8 = 1 + 2
@no-comptime @inline fn runtime_value() -> u8 { return 7 }
```

Ports name an I/O port. Use `out PORT, value` to write and `in PORT` to read.

```ezra
port DEBUG: u8 = 0x0C

fn main() {
    out DEBUG, 'A'
    let status: u8 = in DEBUG
}
```

MMIO declarations name memory-mapped addresses. Add `volatile` when the location has hardware side effects or must not be optimized as ordinary memory.

```ezra
volatile mmio FRAMEBUFFER: ptr<u8> = 0x080000

fn main() {
    *FRAMEBUFFER = 0xFF
}
```

## Embedded Data

`embed` places immutable byte data into the program image. The legacy `bytes` spelling remains available. A typed embed must be a fixed `u8` array whose declared length exactly matches the embedded bytes.

```ezra
embed logo: bytes = file("assets/logo.bin")
embed message: bytes = text("HELLO")
embed c_message: bytes = cstr("HELLO")
embed palette: bytes = bytes [0x00, 0x11, 0x22, 0x33]
embed padding: bytes = repeat(0, 256)

embed matrix: [u8; 4] = [1, 2, 3, 4]
```

Embeds may select an output section and alignment.

```ezra
embed banked: [u8; 2] = [0xA1, 0xA2] section .bank1 align 256
```

Use custom layouts to define additional sections. A project can also provide
portable default placement and target-specific overrides without changing the
source declaration:

```toml
[assets]
section = ".assets"
align = 16

[assets.targets."gameboy-*"]
section = ".rodata"
align = 16

[assets.targets."zxspectrum-*"]
section = ".assets"
align = 256

[assets.targets."agonlight-*"]
section = ".assets"
align = 64
```

Target patterns accept one `*` wildcard. Explicit `section` or `align` clauses
on an `embed` declaration take precedence over project defaults. Layouts and
packagers then decide what those sections mean: cartridge ROM for Game Boy,
tape/image sections for ZX Spectrum, or dedicated asset memory for Agon and
other mapped targets. The source-facing symbols remain stable across targets.

The CLI can preprocess selected indexed PNG file embeds into target-native
image bytes. The declaration remains an immutable byte embed:

```toml
[[assets.images]]
path = "assets/player.png"
kind = "sprite"
```

```ezra
embed player: bytes = file("assets/player.png")
```

See [Indexed PNG image assets](image-assets.md) for formats and limits. This is
project preprocessing, not new language syntax. Unconfigured files are embedded
unchanged.

## Structs

Structs group named fields.

```ezra
struct Point {
    x: u8
    y: u8
}

global origin: Point = Point { x: 0, y: 0 }
```

Access fields with `.`. Address-of works for simple fields and nested access paths.

```ezra
let x: u8 = origin.x
let ptr: ptr<u8> = &origin.x
```

## Functions

Functions use explicit parameter and return types. Omit the return type for functions that return no value.

```ezra
fn add(a: u8, b: u8) -> u8 {
    return a + b
}

fn clear() {
    return
}
```

A function has zero, one, or two ordered scalar results. Two results use a comma-separated return type and a matching two-place `let`:

```ezra
import ezra.int

fn divide(value: u16, divisor: u16) -> u16, u16 {
    let quotient: u16, remainder: u16 = int.divmod(value, divisor)
    return quotient, remainder
}
```

`return a, b` is the two-value return form. A two-result call is not a tuple or a general expression: consume it with a matching two-place binding or return it directly from a matching two-result function. Every return path must provide the declared number and types of results. Arrays, `bytes`, strings, structs, tuples, and other aggregate or large values are not multi-value results; pass them through pointers instead.

Result locations belong to the selected target ABI. Current backends use different layouts, including a normal first result with caller-provided storage for a second result, paired registers, or target-specific secondary-result registers. Do not rely on a layout across targets. Unsupported result types, result combinations, extern declarations, or ABI forms produce diagnostics.

Function modifiers and attributes may appear in either order before `fn`:

```ezra
pub @inline fn helper() -> u8 { return 1 }
@inline pub fn exported_helper() -> u8 { return 2 }
@extern pub fn binary_api() -> u8 { return 3 }
naked fn interrupt_entry() {}
interrupt fn timer_isr() {}
```

`@inline` records the `inline` function attribute. The legacy `inline fn` spelling remains supported and normalizes to the same attribute, so the two spellings should not be combined on one function. The attribute requests inlining when the target backend can safely expand the function. Backends may also inline automatically when their target cost model determines that the function body is cheaper than the call, prologue, return, and associated state preservation. Recursive calls and unsupported body shapes fall back to ordinary calls.

`@comptime` marks a pure function for compile-time evaluation when its inputs are known. `@no-comptime` takes precedence over both compile-time evaluation and inlining. `pub` controls source and module visibility but does not keep an unused declaration in the final binary. `@extern` marks a function as part of the binary API and keeps it as a linker/emission root. Supported modifiers and attributes are `pub`, `@inline` (or legacy `inline`), `@comptime`, `@no-comptime`, `@extern`, `naked`, and `interrupt`. Backend support for ABI-sensitive modifiers is target-dependent and still evolving.

External assembly functions declare routines implemented by emitted or linked assembly.

```ezra
extern asm fn read_status() -> u8
pub extern asm fn put_char(ch: u8)
```

### Function Pointers

A typed function pointer records its parameter types and optional result type. Take a function address with `&name`, store it in a global or local, pass it as a parameter, and call it like a direct function.

```ezra
fn add(left: u8, right: u8) -> u8 {
    return left + right
}

fn invoke(callback: ptr<fn(u8, u8)u8>) -> u8 {
    return callback(20, 22)
}

fn main() {
    let callback: ptr<fn(u8, u8)u8> = &add
    let answer: u8 = invoke(callback)
}
```

The result type follows the closing parameter parenthesis without `->`. `ptr<fn()>` and `ptr<fn(u8)>` are void-return function pointers. Assignments and calls require an exact compatible signature. Function pointers currently support zero or one result; two-result callbacks are rejected.

The selected target defines the pointer representation and indirect-call sequence. Some targets point directly to the function entry, while targets with static argument slots use compiler-generated trampolines. PIC18 keeps data pointers and code addresses separate and lowers indirect calls through `CALLW`. LR35902 rejects banked callback targets, and WDC 65C816 callbacks are limited to code in bank `$00`. Code that depends on a specific code-address representation or calling sequence is not portable between targets.

## Intrinsic Catalog

The following catalog entries are implemented. `T` means one exact integer type; `U` and `V` may differ in width but must follow the rule shown. `same` means the result keeps the value's exact type.

### `ezra.bits`

These operations accept unsigned `u8`, `u16`, or `u24` values. Index, offset, width, and rotate-count arguments are unsigned integer values where noted.

```text
bits.rotate_left(value, count) -> same
bits.rotate_right(value, count) -> same
bits.test(value, index) -> bool
bits.set(value, index) -> same
bits.clear(value, index) -> same
bits.toggle(value, index) -> same
bits.extract(value, offset, width) -> same
bits.insert(base, value, offset, width) -> same
bits.byte_swap(u16 | u24) -> same
bits.reverse(value) -> same
bits.count_ones(value) -> u8
bits.leading_zeros(value) -> u8
bits.trailing_zeros(value) -> u8
```

`rotate_left` and `rotate_right` reduce the count modulo the value width. Bit indexes must be compile-time constants in `0..width`; `extract` and `insert` require compile-time `offset` and positive `width` with `offset + width <= value width`. `insert` requires `base` and `value` to have the same exact type. Signed values and unsupported widths diagnose.

### `ezra.int`

```text
int.widening_mul(U, V) -> wider exact integer
int.mul_high(T, T) -> T
int.saturating_add(T, T) -> T
int.saturating_sub(T, T) -> T
int.divmod(T, T) -> T, T
int.add_carry(T, T, bool) -> T, bool
int.sub_borrow(T, T, bool) -> T, bool
int.full_mul(T, T) -> T, T
```

`widening_mul` requires matching signedness and a product width of 16, 24, or 32 bits; for example, `u8 * u8 -> u16` and `u16 * u8 -> u24`. The other integer operations require the same exact integer type for both operands. `mul_high` returns the high half, `full_mul` returns low then high halves, and `divmod` returns quotient then remainder. `add_carry` and `sub_borrow` return the width-limited result and a mathematical carry or borrow flag.

Width-limited integer results use EZRA's defined arithmetic: unsigned and signed values wrap at their declared width, signed division truncates toward zero, remainder has the dividend's sign, and division or remainder by zero produces zero. `widening_mul` returns its full wider product, `full_mul` exposes that product as low then high halves, and saturating operations clamp to the signed or unsigned representable bound. A target may reject a legal catalog type or combination when its scalar width or lowering does not support it.

### `ezra.mem`

All pointer arguments below are `ptr<u8>` and all lengths are `u24`.

```text
mem.copy_nonoverlapping(destination, source, length)
mem.move(destination, source, length)
mem.fill(destination, value: u8, length)
mem.find_byte(data, length, value: u8) -> ptr<u8>, bool
mem.compare(left, right, length) -> i8

mem.load_le16(address) -> u16
mem.load_le24(address) -> u24
mem.load_be16(address) -> u16
mem.load_be24(address) -> u24
mem.store_le16(address, value: u16)
mem.store_le24(address, value: u24)
mem.store_be16(address, value: u16)
mem.store_be24(address, value: u24)

mem.peek8(address) -> u8
mem.poke8(address, value: u8)
```

`copy_nonoverlapping` requires source and destination ranges not to overlap; a statically known overlap is an error. `move` permits overlap and copies as if the source were preserved, choosing forward or backward order. `fill` writes one byte repeatedly. `find_byte` returns the matching address and `true`, or `data + length` and `false`; `compare` returns exactly `-1`, `0`, or `1` from an unsigned bytewise comparison. The endian loads and stores access exactly 2 or 3 bytes in the named order, independent of target endianness, and define unaligned ordinary-memory behavior where the selected target supports the operation.

The compatibility spellings `mem.memcpy` and `ezra.mem.memcpy` name `copy_nonoverlapping`; `mem.memset` and `ezra.mem.memset` name `fill`.

Block memory, search, comparison, and endian load/store intrinsics require ordinary nonvolatile memory. They do not turn MMIO into safe RAM or combine device accesses. `mem.peek8` and `mem.poke8` preserve one explicit byte access and may be used for volatile/MMIO byte locations when the device semantics permit it; use a target SDK operation for device-specific read-modify-write behavior.

Two-result intrinsic calls (`divmod`, `add_carry`, `sub_borrow`, `full_mul`, and `find_byte`) use the same two-place binding rules as user functions. Zero-result calls are statements, not values.

## Statements

Local variables require a type and initializer.

```ezra
let i: u8 = 0
```

Assignment supports ordinary and compound operators.

```ezra
i = 1
i += 1
i -= 1
i *= 2
i /= 2
i %= 2
i &= 0x0F
i |= 0x80
i ^= 0xFF
i <<= 1
i >>= 1
```

Control flow uses blocks.

```ezra
if value == 0 {
    return
} else if value < 10 {
    value += 1
} else {
    value = 0
}

while value < 10 {
    value += 1
}

loop {
    break
}
```

`break` and `continue` are valid in loops. `return` may be empty, return one expression, or return two comma-separated expressions when the function signature matches.

## Expressions

Supported expression forms include:

```ezra
name
module.name
function(arg1, arg2)
array[index]
object.field
object.nested[index].field
[1, 2, 3]
TypeName { field: value }
&name
&array[index]
&object.field
*pointer
cast<u16>(value)
in PORT
```

Operator precedence, from highest to lowest:

```text
unary:          -  ~  !
multiplicative: *  /  %
additive:       +  -
shift:          <<  >>
comparison:     <  <=  >  >=
equality:       ==  !=
bitwise and:    &
bitwise xor:    ^
bitwise or:     |
logical and:    &&
logical or:     ||
```

Use parentheses to make mixed arithmetic and bit operations explicit.

## Inline Assembly

Inline assembly is a statement. Each assembly line is a string literal.

```ezra
asm {
    "nop"
    "ret"
}
```

Add `volatile` for assembly with side effects.

```ezra
asm volatile {
    "ei"
}
```

Operands document inputs, outputs, and clobbers for the compiler.

```ezra
asm volatile (
    in value: u8 as reg8,
    out result: u8 as reg8,
    clobber af,
) {
    "ld a, 1"
}
```

Operand classes are `reg8`, `reg16`, `reg24`, `mem`, and `imm`. Current inline assembly lowering is intentionally simple; prefer target SDK functions for reusable hardware access.

## Layout Files

Layout files use `.ezralayout` and describe memory regions, sections, and symbols.

```ezra
layout demo {
    load 0x010000
    entry 0x010040
    stack 0x0FFF00

    region code 0x010000..0x03FFFF read execute
    region rodata 0x040000..0x04FFFF read
    region ram 0x050000..0x0BFFFF read write
    region stack 0x0F0000..0x0FFFFF read write reserved

    section .text -> code align 16
    section .rodata -> rodata align 16
    section .data -> ram align 16

    symbol EZRA_LOAD_ADDR = 0x010000
    symbol EZRA_ENTRY_ADDR = 0x010040
    symbol EZRA_STACK_TOP = 0x0FFF00
}
```

Region flags are `read`, `write`, `execute`, `volatile`, and `reserved`. Layout addresses are validated against the selected target address width.

## Practical Style

Use explicit widths at hardware boundaries, address literals, and SDK calls. Keep platform-specific declarations behind `@cfg` predicates. Prefer public functions and constants in module files, and keep private helper declarations unmarked.
