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
cpu("ez80" | "z80" | "z80n" | "z180" | "i8080" | "i8085" | "lr35902")
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
```

Other built-in names used by the compiler and SDK include `bool` and `bytes`. Paths may also name structs or aliases.

Pointers use `ptr<T>` and arrays use `[T; LEN]`.

```ezra
alias Byte = u8

global counter: u8 = 0
global buffer: [u8; 16] = [0, 0, 0, 0]
global framebuffer: ptr<u8> = 0x080000
```

The target controls pointer width. eZ80, WDC 65C816, and the generic M68k target use 24-bit pointers. Z80-family, 8080/8085, LR35902, MOS 6502, TMS9900, and AVR targets use 16-bit pointers.

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

Strings use double quotes.

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

`embed` places byte data into the program image. The declared type is currently `bytes`.

```ezra
embed logo: bytes = file("assets/logo.bin")
embed message: bytes = text("HELLO")
embed c_message: bytes = cstr("HELLO")
embed palette: bytes = bytes [0x00, 0x11, 0x22, 0x33]
embed padding: bytes = repeat(0, 256)
```

Embeds may select an output section and alignment.

```ezra
embed banked: bytes = bytes [0xA1, 0xA2] section .bank1 align 256
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

Function modifiers and attributes may appear in either order before `fn`:

```ezra
pub @inline fn helper() -> u8 { return 1 }
@inline pub fn exported_helper() -> u8 { return 2 }
naked fn interrupt_entry() {}
interrupt fn timer_isr() {}
```

`@inline` records the `inline` function attribute. The legacy `inline fn` spelling remains supported and normalizes to the same attribute, so the two spellings should not be combined on one function. The attribute requests inlining when the target backend can safely expand the function. Backends may also inline automatically when their target cost model determines that the function body is cheaper than the call, prologue, return, and associated state preservation. Recursive calls and unsupported body shapes fall back to ordinary calls.

Supported modifiers and attributes are `pub`, `@inline` (or legacy `inline`), `naked`, and `interrupt`. Backend support for ABI-sensitive modifiers is target-dependent and still evolving.

External assembly functions declare routines implemented by emitted or linked assembly.

```ezra
extern asm fn read_status() -> u8
pub extern asm fn put_char(ch: u8)
```

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

`break` and `continue` are valid in loops. `return` may return an expression only from functions with a return type.

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
