# Tests, debug helpers, and memory intrinsics

## Built-in test helpers

The test SDK exposes assertions and pass/fail helpers to programs run by `ezrac test`:

```ezra
fn main() {
    test.assert_eq_u8(2 + 3, 5, 1)
    test.pass()
}
```

The exact helper set is target-harness-specific. Test programs must be run with an `ezra-test-*` target or another target whose SDK provides the helper. See the checked-in fixtures under `tests/fixtures/harness` and [the CLI test command](../cli.md#test).

## Debug helpers

The eZ80 test/debug support also provides:

```text
debug.char(value)
debug.str(text)
debug.hex_u8(value)
debug.hex_u16(value)
debug.hex_u24(value)
```

These helpers are target/runtime helpers, not portable output functions. Use the target SDK for user-facing device output.

## Memory helpers

`ezra.mem` accepts `ptr<u8>` addresses and `u24` lengths for its byte-oriented operations:

```text
copy_nonoverlapping(destination, source, length)
move(destination, source, length)
fill(destination, value, length)
find_byte(data, length, value) -> ptr<u8>, bool
compare(left, right, length) -> i8
load_le16/load_le24/load_be16/load_be24
store_le16/store_le24/store_be16/store_be24
peek8(address) -> u8
poke8(address, value)
```

`copy_nonoverlapping` rejects known overlap; `move` allows overlap. `find_byte` returns `data + length` and `false` when it does not find a match. `compare` returns `-1`, `0`, or `1`. Endian operations access exactly the named number of bytes.

Block memory operations require ordinary nonvolatile memory. `peek8` and `poke8` preserve one explicit byte access and may be used for suitable MMIO locations; they do not replace a target device protocol.

## Integer and bit helpers

`ezra.bits` requires unsigned `u8`, `u16`, `u24`, or another explicitly supported target width for values for rotates, bit selection, extraction, insertion, reversal, and bit counts. `ezra.int` provides widening multiplication, high multiplication, saturation, division/remainder, carry/borrow, and full multiplication. Operations use exact widths and can return two scalar results.

## Debugging generated code

Use `--debug-comments`, `emit-ir`, and `emit-asm` to inspect a failure. A target can reject a legal catalog call when its scalar width, ABI, or emitter cannot lower it. The [troubleshooting guide](../diagnostics-and-troubleshooting.md) gives the recommended command order.
