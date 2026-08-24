# Getting started

## Prerequisites

You need:

- Rust and Cargo for a source checkout. The repository pins the toolchain in [`rust-toolchain.toml`](../rust-toolchain.toml).
- A target emulator or device only when you want to run a produced image. Compilation and tests do not need target hardware.
- PowerShell and a local Fab Agon Emulator checkout for the Agon tutorial runner.

The workspace has two packages:

- `ezra-core` — compiler library at the repository root.
- `ezrac-cli` — the `ezrac` binary, LSP, and editor installer in `crates/ezrac-cli`.

## Build the compiler

From the repository root:

```sh
cargo build
cargo test --quiet
```

Install the CLI from the checkout when you want the `ezrac` command:

```sh
cargo install --path crates/ezrac-cli
```

Add `--features lsp` if you also need the language server:

```sh
cargo install --path crates/ezrac-cli --features lsp
```

Until the binary is installed, replace `ezrac` in the examples with `cargo run --`.

## First source build

The Agon hello example is a checked-in project:

```sh
cargo run -- check examples/agon-mos/hello/src/main.ezra
cargo run -- build examples/agon-mos/hello/src/main.ezra
```

The project configuration selects `agonlight-mos-ez80`. The output is placed under:

```text
examples/agon-mos/hello/target/agonlight-mos-ez80/hello.bin
```

Use [the Agon tutorial](tutorials/agon-mos-coffee-order.md) to run an image in Fab Agon Emulator.

## Inspect a program

These commands operate on a source file:

```sh
cargo run -- check examples/agon-mos/hello/src/main.ezra
cargo run -- emit-asm examples/agon-mos/hello/src/main.ezra
cargo run -- emit-ir --stage hir examples/agon-mos/hello/src/main.ezra
cargo run -- emit-ir --stage tbir examples/agon-mos/hello/src/main.ezra
cargo run -- build examples/agon-mos/hello/src/main.ezra
```

`check` validates without writing an executable. `emit-asm` prints validated target assembly. `emit-ir` prints an intermediate representation. `build` writes the generated assembly, map, size report, and executable.

## Run compiler tests

Run a single checked-in VM test fixture:

```sh
cargo run -- test --target ezra-test-flat-ez80 tests/fixtures/harness/flat_complex.ezra
```

Run the Rust test suite after changing compiler code:

```sh
cargo test --quiet
```

The VM test command is not a general-purpose emulator for every target. It currently uses the built-in `ez80` emulator for the eZ80/Z80-family test profiles. See [CLI usage](cli.md#test) for details.

## Assemble a file

Build a raw assembly example through the target pipeline:

```sh
cargo run -- assemble --target cpm-2.2-z80 --map console-output.map examples/cpm-z80/console-output.asm
```

See [Assembly usage](assembly.md) for direct output, base addresses, macros, and limitations.
