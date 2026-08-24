# Modules, imports, aliases, and visibility

## Imports

`import` loads another `.ezra` file or a built-in SDK/intrinsic module:

```ezra
import math
import agon.console
import ezra.mem
```

For `import foo.bar`, the compiler searches for `foo/bar.ezra` relative to the importing file, ancestor directories, the working directory, configured `[sdk].paths`, and finally the target's bundled SDK. Imports resolve recursively; cycles are rejected and duplicate imports are de-duplicated.

Public imported names are available through the full path and, when unambiguous, a short alias:

```ezra
import agon.console

fn main() {
    console.print("hello")
    agon.console.newline()
}
```

## Visibility

Top-level declarations are private by default. Use `pub` for module exports:

```ezra
pub const WIDTH: u16 = 320

pub fn draw() {}

struct PrivateState {
    value: u8
}
```

Visibility controls source access. It does not by itself keep an unused function in the final binary; use `@extern` when a declaration is an external emission root.

## Aliases

Type aliases give a type another name:

```ezra
alias Byte = u8
alias Pixel = struct { value: u8 }
```

Aliases do not create a new runtime representation. The underlying type's width, pointer rules, and target restrictions still apply.

## Intrinsic modules

The compiler supplies three non-editable catalogs:

- `ezra.bits` — bit selection, extraction, insertion, reversal, and counts.
- `ezra.int` — defined-width arithmetic and two-result operations.
- `ezra.mem` — memory copy, fill, compare, search, byte access, and endian loads/stores.

See [SDK concepts](../sdk.md) and [language diagnostics](diagnostics.md) for their contracts.
