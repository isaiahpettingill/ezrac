# Tutorial: hello and a test

This tutorial starts with the smallest executable shape and then points it at the repository's test harness.

## The program shape

An executable source file needs a no-argument, no-result entry point:

```ezra
fn main() {
    test.pass()
}
```

`test.pass()` is supplied by the test harness SDK. It is not a portable function available on every target.

## Build and inspect a checked-in hello program

The repository's smallest normal target example is `examples/agon-mos/hello`:

```sh
cargo run -- check examples/agon-mos/hello/src/main.ezra
cargo run -- emit-asm examples/agon-mos/hello/src/main.ezra
cargo run -- build examples/agon-mos/hello/src/main.ezra
```

The project file selects `agonlight-mos-ez80`; the build writes `.asm`, `.map`, `.size`, and `.bin` files under `examples/agon-mos/hello/target/agonlight-mos-ez80/`.

## Run a harness test

Use the checked-in complex harness fixture to exercise assertions, globals, arrays, structs, embeds, and inline assembly:

```sh
cargo run -- test --target ezra-test-flat-ez80 tests/fixtures/harness/flat_complex.ezra
```

A passing run reports the harness result. The target VM path is an eZ80 emulator; it is not a test of every hardware SDK. For more test helpers and failure isolation, read [language diagnostics](../language/diagnostics.md) and [CLI test](../cli.md#test).
