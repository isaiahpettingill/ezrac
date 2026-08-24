# Tutorial: assembly-only program

Use this path when the source language is not needed and you want to control the instructions directly.

## Build the checked-in CP/M assembly example

From the repository root:

```sh
cargo run -- assemble --target cpm-2.2-z80 --map console-output.map examples/cpm-z80/console-output.asm
```

This assembles the file into a raw image and writes a map beside the current working directory. To use the target's normal `.com` packaging and project output directory instead:

```sh
cargo run -- build --target cpm-2.2-z80 --input-kind assembly examples/cpm-z80/console-output.asm
```

The source file is the same one used by the [CP/M examples](../../examples/cpm-z80/README.md).

## Add a base address

A raw assembly program can be assembled at an explicit address:

```sh
cargo run -- assemble --target bare-z80 --cpu z80 --base 0x0100 --output program.bin examples/cpm-z80/console-output.asm
```

`--base` affects label addresses and relocation. Use the target's documented load address when the image is going to an emulator or device.

## What the assembler does not do

The assembler validates syntax, operands, CPU-specific instructions, labels, includes, and macros. It does not compile EZRA source or infer an SDK ABI. For reusable target APIs, write assembly routines or vendorable macros and document their calling convention. Check [opcode coverage](../assembly.md#cpu-selection-and-coverage) before using a less common instruction.
