# Types, literals, and casts

## Primitive types

The source language defines explicit integer widths:

```text
u8   i8
u16  i16
u20  i20   (target-dependent)
u24  i24
u32  i32   (target-dependent)
bool
bytes
```

Structs, aliases, pointers, and arrays are also types. `bytes` names immutable byte storage used by embeds and strings.

Pointers use `ptr<T>` and arrays use `[T; LENGTH]`:

```ezra
let address: ptr<u8> = 0x080000u24
global pixels: [u8; 16] = [0, 0, 0, 0]
```

Pointer width is target-defined. eZ80, WDC 65C816, and generic M68k use 24-bit pointers; many Z80-family, 6502, AVR, and base MSP430 targets use 16-bit pointers. Integer width and pointer width are separate.

## Literals

```ezra
let decimal: u8 = 42
let hex: u16 = 0x2A
let binary: u8 = 0b101010
let forced: u24 = 0x040045u24
let signed: i8 = -2i8
let yes: bool = true
let letter: u8 = 'A'
let text: bytes = "hello\n"
```

Integer suffixes are `u8`, `i8`, `u16`, `i16`, `u20`, `i20`, `u24`, `i24`, and target-supported wider forms. `u20`/`i20` are used by targets such as MSP430X and remain target-dependent. Strings are immutable byte storage. Supported escapes are `\n`, `\0`, `\t`, `\\`, `\'`, and `\"`.

## Casts

Use explicit casts when the destination width or pointer type matters:

```ezra
let word: u16 = cast<u16>(byte)
let raw: ptr<u8> = cast<ptr<u8>>(&pixels)
```

Casts do not make an unsupported ABI or target operation valid. The selected backend still checks the resulting type and address width.

## Operators

Unary operators are `-`, `~`, and `!`. Arithmetic and comparisons use the usual `* / %`, `+ -`, shifts, comparisons, equality, bitwise `& ^ |`, and logical `&& ||` precedence. Use parentheses when mixing arithmetic and bit operations.
