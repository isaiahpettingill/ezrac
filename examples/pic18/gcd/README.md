# PIC18 GCD

Build a classic PIC18 Intel HEX image with:

```sh
ezrac build
```

Or from an EZRAC checkout:

```sh
cargo run -- build --target generic-pic18-bare examples/pic18/gcd/src/main.ezra
```

The generic target provides CPU code generation and vectors but no device-specific peripherals or configuration words.
