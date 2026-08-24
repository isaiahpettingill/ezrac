# HIR and TBIR

HIR and TBIR separate source-language meaning from target-specific code generation.

## HIR

HIR is typed and target-independent. It carries declarations, control flow, constants, pointer and aggregate types, imports, visibility, and source locations after parsing and semantic checks. A legal HIR program can still be rejected later if a target cannot represent its widths or ABI.

## TBIR

TBIR lowers HIR into operations that are close to a target backend. It chooses scalar widths, pointer operations, call and return shapes, memory access classes, runtime helpers, and target-specific constraints. Optimization passes run over TBIR. The optimization level can enable scalar cleanup, propagation, loop-invariant code motion, known-bits cleanup, inlining, tail calls, tail recursion, and other passes.

Two-result functions show why this split matters. Source code can write `-> T, U`, but TBIR and the target ABI decide whether the second result uses a register, caller-provided storage, or another target-specific location. Aggregate results are not generalized into this feature; pass arrays and structs through pointers.

## Inspecting and debugging

```sh
cargo run -- emit-ir --stage hir --target custom-unknown-ez80 path/to/main.ezra
cargo run -- emit-ir --stage tbir --target custom-unknown-ez80 path/to/main.ezra
cargo run -- emit-asm --target custom-unknown-ez80 path/to/main.ezra
```

If HIR is valid and TBIR fails, the construct is usually outside the target backend's supported ABI or operation set. If TBIR is valid and assembly fails, inspect the target instruction or layout path.

Implementation details in modules outside the documented `ezra::api` surface can change without compatibility guarantees; the project is pre-1.0.
