# Remaining Work

This is a handoff checklist for continuing toward the full EZRA language goal:

- Implement the complete language specification in `spec.md`.
- Keep all tests passing.
- Compile full programs to readable target assembly.
- Continue maturing the implemented HIR → TBIR → target-emitter pipeline across CPU families.
- Run generated programs on target-appropriate VM and emulator test paths.
- Preserve defined behavior: no undefined arithmetic behavior, divide/remainder by zero produce zero, and signed division truncates toward zero.

## Current State

The project has a working Pest parser; HIR and TBIR pipeline; target emitters for eZ80, LR35902, MOS 6502, and optional M68k; cartridge/map generation; layout validation; import/module support; inline asm validation; and VM/emulator-backed test paths. The eZ80 VM assembler remains actively expanded so more real inline assembly snippets can be assembled and executed in tests.

Recent VM assembler coverage includes:

- `RETN`
- all conditional `RET`, `JP`, and `CALL` mnemonics
- `RST`
- `EXX`
- `SCF`, `CCF`, `CPL`, `DAA`, `NEG`
- accumulator rotate shorthands: `RLCA`, `RLA`, `RRCA`, `RRA`
- `BIT`, `SET`, and `RES` register forms

`optimizations.md` contains phase-2 optimization candidates and IR notes. Treat it as remaining design work to evaluate, not an accepted implementation plan.

## High-Priority Remaining Work

1. Audit `spec.md` section by section against implementation.

   For each section, record whether parser, semantic checks, codegen, cartridge packing, VM execution, and diagnostics have direct tests. Treat untested behavior as unfinished even if it looks implemented.

2. Expand full-program VM tests for every language construct.

   The goal is not only byte-level assembler coverage. Each construct in the language should have at least one compiled EZRA program that emits assembly and runs successfully in the VM:

   - imports and visibility
   - aliases
   - constants and constant evaluation
   - globals
   - arrays
   - structs
   - pointers
   - volatile MMIO
   - ports
   - casts
   - control flow
   - functions and calling convention
   - inline asm
   - naked functions
   - interrupts
   - embeds and assets
   - custom layouts

3. Finish diagnostics from `spec.md` section 40.

   Confirm that each required diagnostic has a test that checks a useful message and, where applicable, source location:

   - type mismatch
   - unknown identifier
   - duplicate declaration
   - invalid cast
   - pointer arithmetic on non-pointers
   - array index out of bounds when known at compile time
   - struct field does not exist
   - inline asm output type mismatch
   - inline asm undeclared clobber
   - layout region overlap

4. Continue expanding VM assembler coverage where inline asm validation recognizes instructions.

   The VM assembler is still a subset. Prioritize instructions that:

   - are accepted or analyzed by inline asm clobber inference
   - appear in the eZ80 manual examples
   - are likely in TI-84 Plus CE or Agon Light style SDK code
   - are useful for testing compiler output

5. Implement the metadata-generated production assembler path.

   The design decision is recorded in `spec.md`: EZRA assembly is a supported production path with EZRA-specific, documented syntax. The assembler should be generated from instruction metadata rather than maintained as a large hand-coded opcode matcher. Follow-up work should:

   - define instruction metadata format
   - generate parser/encoder tables from metadata
   - generate opcode coverage documentation and golden encoding tests
   - feed inline-asm validation and clobber inference from the same metadata where possible
   - keep `ezra assemble` documented and stable as a standalone CLI path

6. Mature the implemented HIR/TBIR backend boundary.

   The compiler now lowers source through HIR and target-bound TBIR before target-specific emitters. MOS 6502 and optional M68k source emitters demonstrate that boundary, but TBIR still retains source-shaped statements and expressions rather than fully lowered basic blocks or machine operations.

   Follow-up work should:

   - lower checked TBIR into typed basic blocks with explicit locals, globals, loads, stores, calls, branches, widths, signedness, and side effects
   - model volatile memory, port I/O, inline asm, memory clobbers, and port clobbers as explicit ordering barriers
   - make target traits for pointer width, endianness, integer lowering, register classes, calling convention, stack alignment, section layout, and runtime helper ABI explicit and consistently consumed by emitters
   - migrate the eZ80 emitter further onto those target abstractions
   - add target-specific assembler, golden-output, and emulator tests before raising a target's support level

7. Continue hardening classic Z80 target modes that exclude eZ80 ADL-only features.

   Classic Z80 profiles such as `zxspectrum-z80` and `bare-z80` exist. Keep their smaller machine model explicit and independently tested:

   - 16-bit address space and 16-bit pointers only
   - no `u24`/`i24` pointer-sized assumptions in the target ABI
   - no eZ80 ADL register, stack, or instruction forms
   - no default memory layout regions above `0xFFFF`
   - target-specific cartridge/header expectations for classic Z80 carts
   - diagnostics when code, layouts, inline asm, embeds, or SDK symbols require eZ80/ADL features
   - golden assembly and VM/emulator tests that prove classic Z80 output is independent from the eZ80 path

8. Expand conditional compilation coverage for multi-target shared code.

   Declaration-level `@cfg(...)` is implemented and filters inactive declarations before name resolution and type checking. Extend tests and documentation as conditional support grows to cover target SDKs and applications selecting imports, constants, embeds, functions, layouts/startup glue, and inline asm by target triple, CPU family, pointer width, target features, SDK features, and user build features. Builds should continue to record the active target/features for reproducibility.

## Medium-Priority Work

- Add a machine-readable spec coverage table, possibly `SPEC_COVERAGE.md`.
- Add more CLI tests for custom layouts, maps, symbols, and cartridge outputs.
- Add negative tests for duplicate or colliding declarations across module imports.
- Add more target-SDK style tests for TI-84 Plus CE and Agon Light hardware abstractions.
- Keep hardware support generic: SDK modules should be ordinary EZRA code over ports/MMIO, not hardcoded compiler behavior.
- Evaluate `optimizations.md`; keep only optimizations that can be proven safe for volatile memory, port I/O, inline asm, and emulator-backed tests.
- Review `spec.md` examples periodically so they remain valid as implementation rules tighten.

## Verification Expectations

Before committing future work, run focused tests for the touched area and then:

```sh
cargo fmt
cargo test --quiet
git diff --check
```

`cargo fmt` currently tends to reflow a couple of existing byte-array assertions in `src/vm.rs`. If that happens, restore only those unrelated formatting hunks before committing.
