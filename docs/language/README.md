# Language overview

EZRA source files use `.ezra`. The language is small, explicit, and target-aware. It has fixed-width integers, direct pointers and I/O, embedded data, inline assembly, and a compiler-provided SDK/intrinsic model.

```ezra
import ezra.int

const LIMIT: u8 = 10

global total: u16 = 0

fn add(left: u8, right: u8) -> u8 {
    return left + right
}

fn main() {
    let value: u8 = add(2, 3)
    total = cast<u16>(value)

    if total < cast<u16>(LIMIT) {
        total += 1
    }
}
```

## Source rules

- Identifiers use ASCII letters, digits, and `_`; the first character cannot be a digit.
- Paths join names with dots, such as `agon.console.print`.
- `//` starts a line comment.
- Semicolons are accepted but usually optional; newlines and block boundaries separate statements.
- Executable source needs `fn main()` with no parameters and no return value. Imported modules can contain a `main`, but it is ignored when imported.
- Top-level declarations are private unless marked `pub`.

## Reference pages

- [Modules, imports, aliases, and visibility](modules-imports.md)
- [Types, literals, and casts](types.md)
- [Constants and compile-time evaluation](constants.md)
- [Globals, ports, and MMIO](globals.md)
- [Functions, returns, and modifiers](functions.md)
- [Control flow and expressions](control-flow.md)
- [Pointers and pointer casts](pointers.md)
- [Arrays and indexing](arrays.md)
- [Structs and field access](structs.md)
- [Inline assembly](inline-asm.md)
- [Conditional compilation and banking](conditional-compilation.md)
- [Embeds and image assets](embeds-assets.md)
- [Tests, debug helpers, and memory intrinsics](diagnostics.md)

The original [single-page language reference](../language.md) includes the same implementation details in one document. The [design specification](../spec.md) describes intended and future behavior; it should not be treated as a list of implemented features.
