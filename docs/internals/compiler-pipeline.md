# Compiler pipeline

The compiler library and CLI use the same pipeline. The CLI resolves host files and project settings; it does not have a separate compiler implementation.

```text
source files
    -> parser / AST
    -> import resolution and cfg filtering
    -> HIR
    -> TBIR and optimization
    -> target assembly
    -> assembler
    -> layout/link
    -> package format or VM image
```

## Stages

1. **Parser** reads `.ezra` source and reports source locations for syntax errors.
2. **Import resolution** loads user modules, configured SDK roots, and bundled target SDKs. Visibility and duplicate/cycle rules are applied here.
3. **HIR** represents typed source operations independently of a machine's registers and instruction syntax.
4. **TBIR** selects target-oriented operations, widths, ABI shapes, memory behavior, and optimization passes.
5. **Assembly emission** produces readable target assembly. `emit-asm` prints only after this assembly passes validation.
6. **Assembly and linking** parse instructions, place sections into the selected `.ezralayout`, resolve symbols, and check region bounds.
7. **Packaging** creates raw binaries, `.com`, `.tap`, ROM, Intel HEX, ELF, or target-specific packages.
8. **VM tests** load a validated image into the supported emulator backend.

Use `emit-ir --stage hir` and `emit-ir --stage tbir` to inspect the middle of the pipeline. Use `.map` and `.size` artifacts to inspect the final placement and size accounting.

## Source versus library builds

The `ezra` library exposes the same compiler, assembler, linker, layout, and package APIs for filesystem-free workspaces. `no_std + alloc` callers provide source and asset files in a `Workspace`; filesystem discovery, CLI, LSP, and the emulator test runner remain host features.
