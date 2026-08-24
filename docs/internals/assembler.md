# Assembler internals

The assembler is used for both handwritten assembly and generated target assembly. It has three broad steps:

1. **Preprocess** relative includes, defines, conditionals, and macros.
2. **Parse and lower** source into a target-independent assembly program with source locations, labels, directives, sections, and instruction operands.
3. **Encode and link** target instructions, resolve expressions and labels, place sections, and emit bytes plus map metadata.

The macro frontend is shared by filesystem and in-memory library callers. It keeps include and invocation locations so diagnostics point back to the originating file.

## Metadata

The semantic assembly model records:

- source files and include relationships;
- labels and expressions;
- sections and alignment;
- directives and data;
- instruction operands and CPU selection;
- source spans for diagnostics;
- symbols and placed addresses for maps.

The linker combines this model with an `.ezralayout`. It checks that encoded bytes fit their section region and that addresses are valid for the target width. The `.map` output exposes the resolved placement.

## CPU backends

CPU parsers and encoders are selected through `AssemblerCpu`. The core set includes 8080, 8085, Z80, R800, Z80N, Z180, eZ80, LR35902, and MOS 6502. Optional features add 8086, AVR, M6800, M68k, MSP430, and TMS9900.

Coverage and syntax differ by backend. Keep the target's opcode coverage page and assembler tests near any change to an encoder. The [`ez80` crate](https://crates.io/crates/ez80) is also used by the VM test path for supported eZ80-family programs.

## Where to test changes

- parser/preprocessor behavior belongs with assembly parser tests;
- instruction encoding belongs with CPU encoder tests;
- placement and maps belong with layout/link tests;
- runtime behavior belongs with VM or optional real-core tests.

Run `cargo test --quiet` before merging compiler or assembler changes. Use `git diff --check` for generated or documentation changes as well.
