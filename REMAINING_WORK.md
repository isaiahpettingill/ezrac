# Remaining Work

This is a handoff checklist for continuing toward the full EZRA language goal:

- Implement the complete language specification in `spec.md`.
- Keep all tests passing.
- Compile full programs to readable eZ80 ADL 24-bit assembly.
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

The working tree may contain an untracked `optimizations.md`; it was intentionally left alone.

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

## Medium-Priority Work

- Add a machine-readable spec coverage table, possibly `SPEC_COVERAGE.md`.
- Add more CLI tests for custom layouts, maps, symbols, and cartridge outputs.
- Add negative tests for duplicate or colliding declarations across module imports.
- Add more target-SDK style tests for TI-84 Plus CE and Agon Light hardware abstractions.
- Keep hardware support generic: SDK modules should be ordinary EZRA code over ports/MMIO, not hardcoded compiler behavior.
- Review `spec.md` examples periodically so they remain valid as implementation rules tighten.

## Verification Expectations

Before committing future work, run focused tests for the touched area and then:

```sh
cargo fmt
cargo test --quiet
git diff --check
```

`cargo fmt` currently tends to reflow a couple of existing byte-array assertions in `src/vm.rs`. If that happens, restore only those unrelated formatting hunks before committing.
