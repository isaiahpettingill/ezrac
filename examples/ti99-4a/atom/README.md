# TI-99/4A Atom Animation

This cartridge demonstrates TMS9918A sprite-table updates: a bright electron sprite travels through the atom's orbit while a second sprite remains as the nucleus.

Build from this directory:

```sh
cargo run --manifest-path ../../../Cargo.toml --features tms9900 -- build
```

The output is `target/ti99-4a-tms9900/ti99-atom.bin`, a raw one-bank cartridge ROM.
