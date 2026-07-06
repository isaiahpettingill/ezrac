# Remaining Work

This is a handoff checklist for continuing toward the full EZRA language goal:

- Implement the complete language specification in `spec.md`.
- Keep all tests passing.
- Compile full programs to readable eZ80 ADL 24-bit assembly.
- Build a real target-neutral IR before treating non-eZ80 backends as supported.
- Run generated programs on the ez80-backed VM test path.
- Preserve defined behavior: no undefined arithmetic behavior, divide/remainder by zero produce zero, and signed division truncates toward zero.

## Current State

The project has a working Pest parser, assembly emitter, cartridge/map generation, layout validation, import/module support, inline asm validation, and an ez80-backed VM test runner. Recent work focused on expanding the VM assembler so more real eZ80 inline assembly snippets can be assembled and executed in tests.

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

5. Reconcile the temporary assembler path with full assembly emission.

   The emitted assembly is the intended artifact. The current test assembler is still a helper subset for VM execution. Decide whether to:

   - keep growing the subset assembler,
   - replace it with a real assembler integration,
   - or formalize the subset as only a test fixture while separately validating emitted assembly with a fuller tool.

6. Introduce a target-neutral middle IR before adding additional CPU backends.

   The current implementation effectively lowers the AST and semantic information directly into eZ80-specific assembly. `src/asm.rs` owns eZ80 register choices, stack layout, helper routines, calling convention details, and assembly syntax. That is workable for the current scaffold, but it is not a reusable backend boundary.

   This IR should be retro-oriented and still needs a dedicated spec. Prioritize Z80, eZ80, 8080, and adjacent 8-bit CPU families, while keeping m68k as a desired future target. It should model 8-bit, 16-bit, and 24-bit addressing directly, leave room for 32-bit addressing as a later extension, and make volatile memory, port I/O, inline asm, memory spaces, register classes, flags, calling conventions, and runtime helper ABI explicit.

   To make a target such as m68k realistic:

   - lower checked EZRA into typed basic blocks with explicit locals, globals, loads, stores, calls, branches, widths, signedness, and side effects
   - model volatile memory, port I/O, inline asm, memory clobbers, and port clobbers as explicit ordering barriers
   - define target traits for pointer width, endian behavior, integer lowering, register classes, calling convention, stack alignment, section layout, and runtime helper ABI
   - rebuild the eZ80 emitter on top of that IR first, then use it as the reference for any m68k backend
   - add target-specific assembler or golden-output tests before claiming support

7. Add a classic Z80 target mode that deliberately excludes eZ80 ADL-only features.

   This should be a separate target profile, not a compatibility promise for the current eZ80 backend. Define and enforce the smaller machine model before implementation:

   - 16-bit address space and 16-bit pointers only
   - no `u24`/`i24` pointer-sized assumptions in the target ABI
   - no eZ80 ADL register, stack, or instruction forms
   - no default memory layout regions above `0xFFFF`
   - target-specific cartridge/header expectations for classic Z80 carts
   - diagnostics when code, layouts, inline asm, embeds, or SDK symbols require eZ80/ADL features
   - golden assembly and VM/emulator tests that prove classic Z80 output is independent from the eZ80 path

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
