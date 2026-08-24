# Functions, returns, and modifiers

## Definitions

Parameters and results have explicit types. Omit the result type for a function that returns no value:

```ezra
fn add(left: u8, right: u8) -> u8 {
    return left + right
}

fn clear() {
    return
}
```

Executable entry points are `fn main()` with no parameters and no result.

## Multiple results

A function can return zero, one, or two ordered primitive results:

```ezra
import ezra.int

fn divide(value: u16, divisor: u16) -> u16, u16 {
    let quotient: u16, remainder: u16 = ezra.int.divmod(value, divisor)
    return quotient, remainder
}
```

Two-result calls must be consumed by a matching two-place `let` or returned directly from a matching function. This is not tuple support. Arrays, strings, structs, and other aggregates must be passed through pointers. Result registers/storage are target ABI details and can differ between backends.

## Function values and indirect calls

Function types can be used through typed function pointers when the selected backend supports indirect calls:

```ezra
fn add_one(value: u8) -> u8 {
    return value + 1
}

fn apply(operation: ptr<fn(u8)u8>, value: u8) -> u8 {
    return operation(value)
}
```

Function-pointer calls must match the declared parameter and result types. The calling convention and indirect-call support remain target-specific.

## Visibility and attributes

Supported modifiers and attributes are `pub`, `@inline` (and legacy `inline`), `@comptime`, `@no-comptime`, `@extern`, `naked`, and `interrupt`:

```ezra
pub @inline fn helper() -> u8 { return 1 }
@extern pub fn binary_api() -> u8 { return 3 }
naked fn entry() {}
interrupt fn timer_isr() {}
```

`@inline` requests inlining but the backend may keep a normal call. `@extern` keeps a function as an emission/linker root. `naked` and `interrupt` are ABI-sensitive and target-dependent; do not assume the same prologue, return instruction, register set, or interrupt support across targets.

## External assembly

Declare a routine supplied by assembly with `extern asm`:

```ezra
extern asm fn read_status() -> u8
pub extern asm fn put_char(ch: u8)
```

The declaration is only a contract. The assembly routine must follow the selected target's ABI.
