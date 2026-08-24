# Assembly usage

EZRA supports handwritten assembly as a first-class input. Use `assemble` when you want a direct raw binary, or `build --input-kind assembly` when the target package and project artifact layout should be used.

## Direct assembly

```sh
ezrac assemble --target cpm-2.2-z80 --cpu z80 --base 0x0100 --map program.map --output program.bin examples/cpm-z80/console-output.asm
```

Addresses accept decimal, `0x` hexadecimal, or `h`-suffixed hexadecimal. `--base` is the address assigned to the first byte and is used for labels. `--map` writes section and symbol information.

From a checkout:

```sh
cargo run -- assemble --target cpm-2.2-z80 --map console-output.map examples/cpm-z80/console-output.asm
```

## Assembly through `build`

```sh
cargo run -- build --target cpm-2.2-z80 --input-kind assembly examples/cpm-z80/console-output.asm
```

The input extension normally selects assembly for `.asm`, `.s`, `.z80`, `.ez80`, `.i8080`, `.8080`, `.i8086`, and `.8086`; use `--input-kind` when the extension or project configuration is not enough.

## Includes and macros

The preprocessor supports relative includes, defines, conditionals, and hygienic macros:

```asm
include "macros/console.inc"
%define DEBUG_PORT 0Ch

%macro print_char(value)
    ld a, $value
    out (${DEBUG_PORT}), a
%endmacro

%if cpu("ez80")
    %print_char 65
%endif
```

Supported conditions are `cpu("name")`, `target("triple")`, `feature("name")`, and `defined(NAME)`. Macro labels beginning with `%%` are private to an invocation. Expansion is recursive up to 32 levels. The macro layer does not compile EZRA SDK functions; reusable assembly APIs must be vendored as assembly macros or routines with documented ABI requirements.

## CPU selection and coverage

The built-in assembler has implemented subsets for 8080, 8085, Z80, R800, Z80N, Z180, eZ80, LR35902, and MOS 6502. Optional Cargo features add strict original 8086, AVR, M6800, M68k, MSP430, and TMS9900 assemblers. Use `--cpu` to select syntax and validation.

Opcode coverage is not uniform. Check the relevant page before relying on an instruction family:

- [eZ80/Z80 coverage](ez80-opcode-coverage.md)
- [8086 assembly](i8086-assembly.md)
- [AVR assembly](platforms.md#avr-and-arduboy)
- [DCPU-16 assembly](dcpu-assembly.md)
- [TMS9900 assembly](tms9900-assembly.md)
- [Other assembly guides](README.md#existing-platform-and-tool-guides)

The assembler rejects unknown instructions, invalid operands, unsupported CPU forms, and out-of-range values. Source compilation also validates the generated assembly before `emit-asm` prints it.

## Emulator parity checks

The built-in `ez80` crate provides the VM path used by `ezrac test` for the eZ80, Z80-family, R800, i8080, and i8085 profiles. Rust tests also cover encoding and execution for optional backends. Third-party libretro tests are opt-in and require local cores; see [real-core tests](real-core-tests.md).

The assembler and VM are not a promise that every target SDK or hardware peripheral is complete. Treat the target support tier and target-specific guide as part of the assembly contract.
