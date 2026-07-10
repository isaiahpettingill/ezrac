# EZRA Specification Coverage

This table audits [`spec.md`](spec.md) against the parser, semantic checks, lowering, assembler, binary layout, VM, and regression tests on `master`.

Status meanings: **Implemented** is exercised end to end by tests; **Partial** has a useful subset with named gaps; **Deferred** waits on a tracked backend project.

| Spec | Status | Evidence | Remaining gap |
| --- | --- | --- | --- |
| 1. Purpose | Implemented | CLI, compiler, assembler, layouts, SDKs, and emulator tests are integrated in `src/main.rs`. | None for the stated purpose. |
| 2. Target and CPU model | Partial | `src/target.rs` models eZ80, Z80, Z80N, Z180, 8080, and 8085 with width checks. | Non-Z80 source backends remain #49–#66. |
| 3. Cartridge kinds | Implemented | `src/cart.rs` and target output selection cover EZRA cartridges and target-owned formats. | Future targets may add formats. |
| 4. Address space | Implemented | `Address24`, layout validation, section-fit, stack, and VM bounds tests enforce current address spaces. | None for current targets. |
| 5. Cartridge header | Implemented | Header serialization and cartridge map/table tests cover fixed fields and absolute addresses. | Reserved fields remain undefined. |
| 6. Layout format | Implemented | `src/layout.rs` parses, validates, places, and reports regions, sections, symbols, and target layouts. | None for current grammar. |
| 7. Project configuration | Implemented | `src/project.rs` and command tests cover target, layout, SDK paths, input kind, artifacts, and scaffolding. | Future backend settings are deferred. |
| 8. Default port map | Implemented | Test/debug/video/audio symbols and ordered port I/O have emitter and VM tests. | Platform SDKs may extend it. |
| 9. Controller layout | Partial | Target SDK input modules exist where supported. | No universal controller runtime exists for every target. |
| 10. Source shape | Implemented | Pest grammar and parser tests cover imports, declarations, attributes, and functions. | None known. |
| 11. Modules | Implemented | Recursive imports, visibility, aliases, collisions, cycles, SDK roots, and qualified names have focused tests. | Incremental workspace caching is LSP work. |
| 12–14. Types, ranges, literals | Implemented | Parser, evaluator, emitter, and diagnostics test integer widths, bool, chars, strings, suffixes, and overflow. | None known for specified types. |
| 15. Constants | Implemented | Forward references, cycles, addresses, casts, wrapping, and storage dependencies are tested. | None known. |
| 16. Type aliases | Implemented | Alias resolution is tested through values, pointers, aggregates, imports, and inline assembly. | None known. |
| 17. Embeds/assets | Implemented | File, inline, text, repeat, typed views, alignment, read-only writes, tables, and relative files are tested. | Compression/streaming is future work. |
| 18. Variables | Implemented | Globals, locals, initialization, assignment, shadow rejection, storage, and volatile behavior are tested. | None known. |
| 19. Arrays | Implemented | Static/nested arrays, bounds diagnostics, indexing, pointers, copies, and array fields are tested. | Runtime bounds checks are not specified. |
| 20. Structs | Implemented | Layout, fields, nested access, addresses, copies, arrays, and pointer arithmetic are tested. | None known. |
| 21. Pointers | Implemented | Address-of, dereference, arithmetic, casts, comparisons, nulls, mutability, and target widths are tested. | Rich provenance optimization remains #15. |
| 22. Volatile memory | Implemented | MMIO lowering and ordering tests prevent dead-code and reorder regressions. | None known. |
| 23. Ports | Implemented | Typed declarations, input/output, target symbols, ordering, and emulator metadata are tested. | Availability remains target-owned. |
| 24–25. Operators/casts | Implemented | Constant/runtime arithmetic, bitwise, shifts, comparisons, signed behavior, pointers, and casts are tested. | None known. |
| 26. Control flow | Implemented | `if`, `while`, `loop`, breaks, continues, returns, dead branches, and reachability are tested. | More TBIR optimization remains #15–#16. |
| 27–29. Functions and ABI | Implemented | Parameters, spills, recursion, returns, frames, inline/naked functions, and extern calls are tested on current backends. | Future CPUs need ABIs. |
| 30. Inline assembly | Implemented | Operand classes, substitution, outputs, clobbers, effects, naked restrictions, CPU gating, and execution are tested. | Exhaustive eZ80 syntax remains #4. |
| 31. Interrupts | Implemented | Signatures, calls, prologues/epilogues, `reti`, and invalid forms are tested. | Vector installation belongs in SDKs. |
| 32–34. Runtime/SDKs | Partial | Agon, TI, CP/M, Spectrum, and harness SDK modules have build/execution coverage. | Complete platform APIs remain #9, #23, #38, #48, #56, and #58. |
| 35. Program entry | Implemented | Main checks, startup, stack, entry addresses, and output wrappers are tested. | None known. |
| 36. Example game | Implemented | Repository examples include complete Agon programs and build fixtures. | Hardware smoke testing is platform-specific. |
| 37. Runtime helpers | Implemented | Arithmetic, memcpy, memset, signed operations, debug, and test helpers execute in VM tests. | Future backends need helper ABIs. |
| 38. Assembly output | Partial | Generated/standalone assembly, CPU modes, global section linking, includes, maps, and formats are tested. | Exhaustive UM0077 enumeration remains #4. |
| 39. Test runner | Partial | Deterministic Z80-family execution supports budgets, memory, ports, traps, stacks, and CI status. | Multi-file architecture-neutral runner remains #59. |
| 40. Compiler pipeline | Partial | AST → HIR → TBIR → assembly → assembler → layout is explicit and dumpable with safe optimization tests. | New backends and advanced passes remain #14–#16 and #49–#66. |
| 41. Diagnostics | Partial | Structured spans, UTF-16 LSP ranges, cross-file multi-reference errors, import/include provenance, and CLI locations are tested. | AST-native semantic spans and full multi-error type checking remain. |
| 42. Grammar sketch | Implemented | `src/ezra.pest` is the executable grammar with parser tests. | Prose must follow grammar changes. |
| 43. Design rules | Partial | Explicit widths, target profiles, strict casts, SDK ownership, readable assembly, and deterministic tests are enforced. | Future backends and broader SDKs remain incomplete. |

## Highest-priority uncovered work

1. Finish AST-native semantic spans and collect independent type errors in one check pass.
2. Complete #54: definitions, symbols, semantic tokens, and dependency/config invalidation.
3. Finish exhaustive eZ80 generated-form enumeration in #4.
4. Build the architecture-neutral project test runner in #59.
5. Move inlining, tail calls, and advanced loop/memory optimization into TBIR passes (#14–#16).

Untested behavior remains incomplete. Validate changes with `cargo test --all-features` and `cargo clippy --all-targets --all-features -- -D warnings` before changing a row to **Implemented**.
