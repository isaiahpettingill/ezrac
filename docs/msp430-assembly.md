# MSP430 Assembly

The optional `msp430` feature enables the MSP430, MSP430X, and MSP430X2
assemblers and source backends. MSP430 targets use little-endian data and the
MSP430X variants can use native 20-bit scalar and pointer operations.

## EZRA source ABI

`AssemblyOptions.stack_top` selects the initial descending stack address. Base
MSP430 accepts a 16-bit address; MSP430X and MSP430X2 accept a 20-bit address.
The emitter aligns the configured value down to an even address and rejects a
value outside the selected target address space.

The source ABI evaluates the primary scalar result in `R4` and a second result
in `R5`. `R1` is the stack pointer and `R13` is the frame pointer. `R4` through
`R12` are caller-clobbered source registers; the allocator may use `R10` through
`R12` for scalar locals and spills them to the active frame when they are live
across calls or inline assembly. A compiled prologue saves `R13`, sets it from
`R1`, and allocates negative, word-aligned local slots. The epilogue releases
the slots, restores `R13`, and uses `RET`.

Parameters are stored in the caller's argument area at positive offsets starting
at `4(R13)`, in source order. Ordinary scalar slots occupy two bytes; native
20-bit values and pointers occupy four bytes on MSP430X targets. Callers allocate
and release argument storage around every call, including indirect calls, so a
callback can re-enter a function without sharing its locals, parameters, or
spills. Frame and indexed displacements are checked before emission.

Aggregate parameters and returns are not part of this source ABI. Use pointers
to caller-owned or frame-backed storage. Explicit globals, MMIO, embeds, and
static SDK state remain fixed-address data. Inline assembly with compiler
operands is unsupported; use compiler-managed storage or a naked wrapper.

Compiler-managed `interrupt` functions are unsupported. Interrupt vectors,
entry register preservation, hardware acknowledgment, and `RETI` must be
provided by a `naked` assembly wrapper. The wrapper must establish or preserve
the normal `R1`/`R13` frame ABI before calling compiled source and must restore
the interrupt state before returning. The emitter does not infer interrupt
entry state or save hardware registers for the wrapper.

## Build

```sh
cargo run --features msp430 -- build --target msp430-none-elf program.ezra
cargo run --features msp430 -- build --target msp430x-none-elf program.ezra
```

The target-specific linker packages MSP430 builds as ELF32. The standalone
assembler accepts the MSP430 instruction forms documented by its target parser;
MSP430X forms use the `.a` suffix where the 20-bit width is required.
