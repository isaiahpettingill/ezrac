# Diagnostics and troubleshooting

EZRA reports diagnostics during parsing, import resolution, type checking, target lowering, assembly, layout validation, and packaging. Start with the narrowest command that reproduces the problem.

## A useful sequence

```sh
cargo run -- check --target <target> path/to/main.ezra
cargo run -- emit-ir --stage hir --target <target> path/to/main.ezra
cargo run -- emit-ir --stage tbir --target <target> path/to/main.ezra
cargo run -- emit-asm --target <target> path/to/main.ezra
cargo run -- build --target <target> path/to/main.ezra
```

- If `check` fails, fix source, imports, types, or configuration first.
- If HIR prints but TBIR fails, inspect target-independent lowering and optimization-sensitive constructs.
- If TBIR prints but assembly fails, the selected backend or ABI does not support the lowered operation.
- If assembly validates but build fails, inspect the layout, section sizes, package format, or target feature.

## Common errors

### Import not found

For `import device.video`, check that one of these exists:

```text
<source directory>/device/video.ezra
<ancestor directory>/device/video.ezra
<working directory>/device/video.ezra
<configured SDK root>/device/video.ezra
```

Built-in SDK modules are searched last. Add a project path in `[sdk].paths` or use the target whose SDK owns the module.

### Declaration is not visible

Top-level declarations are private by default. Add `pub` to exported functions, constants, globals, structs, or embeds:

```ezra
pub const SCREEN_WIDTH: u16 = 256
pub fn draw() {}
```

### Type or width mismatch

EZRA does not silently choose integer widths. Add an explicit suffix or cast:

```ezra
let address: u24 = 0x040045u24
let byte: u8 = cast<u8>(value)
```

The target still may reject a legal source width when its ABI or emitter cannot lower it.

### Pointer and array errors

Arrays are storage and do not decay to element pointers. Use `ptr<[T; N]>` for an array parameter, or explicitly cast the address to `ptr<T>` before pointer arithmetic. See [pointers](language/pointers.md) and [arrays](language/arrays.md).

### Layout overflow

Use `ezrac layout` and inspect the `.map` and `.size` build artifacts. Move a section to a larger region, reduce the asset or code size, or select a target with a suitable layout. Do not solve an overflow by changing the entry address without checking startup and SDK assumptions.

### Assembly instruction rejected

Pass the correct `--cpu`, then check the opcode coverage page for that CPU. A target profile can resolve while its assembler or source emitter still supports only a subset. Optional CPU families also require their Cargo feature.

### Emulator test fails

Confirm the test target is one of the built-in VM profiles and that the program uses the harness SDK correctly. A successful compile is not a hardware or third-party emulator result. See [real-core tests](real-core-tests.md) for opt-in external validation.

## Debugging aids

- `--debug-comments` adds source and lowering comments to generated assembly.
- `emit-ir --stage hir` shows typed, target-independent structure.
- `emit-ir --stage tbir` shows target-oriented operations and ABI choices.
- `emit-asm` shows the validated assembly that the backend hands to the assembler.
- `.map` shows placed sections and symbols.
- `.size` shows section, runtime-helper, address-span, gap, payload, and final-package sizes.

The language-specific limits are collected in [language diagnostics](language/diagnostics.md). Compiler contributors should also read the [pipeline](internals/compiler-pipeline.md).
