# Constants and compile-time evaluation

## Constants

`const` creates a named compile-time value:

```ezra
const MAX_LIVES: u8 = 3
pub const SCREEN_BASE: u24 = 0x080000u24
const TABLE: [u8; 3] = [4, 7, 9]
```

Constant expressions support arithmetic, boolean and bitwise operations, casts, references to other enabled constants, and known indexing into immutable constant arrays. Short fixed-size array initializers are zero-filled; `zeroes()` creates a zero-filled fixed-size array.

```ezra
const OFFSET: u8 = 2 + 3
const ZEROES: [u8; 4] = zeroes()

fn read_table() -> u8 {
    return TABLE[1] + OFFSET + ZEROES[3]
}
```

## `@comptime`

Mark a pure function for compile-time evaluation when all inputs are known:

```ezra
@comptime fn add(left: u8, right: u8) -> u8 {
    return left + right
}

fn answer() -> u8 {
    return add(2, 3)
}
```

The compiler falls back to a runtime call when an input is unknown or evaluation reaches its limit. Mutable globals, ports, MMIO, pointers, strings, assignments, loops, recursion, inline assembly, and other effectful calls are not evaluated as compile-time values.

## `@no-comptime` and inlining

Use `@no-comptime` to prevent constant folding for a value or prevent comptime evaluation and inlining for a function:

```ezra
@no-comptime const RUNTIME_VALUE: u8 = 1 + 2
@no-comptime @inline fn runtime_value() -> u8 { return 7 }
```

`@no-comptime` takes precedence over `@comptime` and inlining. Optimization level and target cost models can still affect ordinary code generation; see [project optimization settings](../projects.md#optimization-settings).
