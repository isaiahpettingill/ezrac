# eZ80 Test Harness Targets

EZRA includes emulator-only eZ80 targets for compiler and runtime integration
tests. These targets are selected through the same `--target` and `Ezra.toml`
paths as real hardware targets, but their machine contract is owned by the
in-repo `ez80` emulator test runner.

## `ezra-test-flat-ez80`

- CPU: eZ80 ADL mode.
- Pointer width: 24 bits.
- Output format: raw `.bin` plus `.asm` and `.map` artifacts.
- Load address: `0x010000`.
- Entry address: `0x010040`.
- Stack top: `0x0FFF00`.
- Memory model: reserved low page, contiguous code/rodata/RAM/assets/scratch,
  high reserved stack region.
- Ports: port `0x0C` captures debug output, port `0x0D` captures test result,
  and writing `1` to port `0x0E` halts the test.

## `ezra-test-split-ez80`

- CPU: eZ80 ADL mode.
- Pointer width: 24 bits.
- Output format: raw `.bin` plus `.asm` and `.map` artifacts.
- Load address: `0x020000`.
- Entry address: `0x020040`.
- Stack top: `0x1FFF00`.
- Memory model: reserved zero page and ROM region, separate code/rodata,
  high RAM/assets/scratch, and high reserved stack region.
- Ports: same test ports as `ezra-test-flat-ez80`.

The harness intentionally traps execution outside the compiled image. Tests use
this to catch bad jumps and startup/entry regressions without depending on MOS
wrappers or an external emulator.

## Built-In Harness SDK

The `ezra-test-*` targets expose a small built-in SDK for integration fixtures:

- `harness.io`: deterministic input ports, debug output, test result, and halt
  helpers.
- `harness.memory`: pointer and volatile-MMIO helpers for flat and split RAM
  addresses.
- `harness.layout`: assertions for the flat and split target layout symbols.

Complex regression fixtures live under `tests/fixtures/harness` and are executed
through the normal CLI build/test paths. They intentionally cover broad target
families and executable formats: eZ80 ADL raw binaries with flat and split memory
layouts, and classic Z80 CP/M `.com` assembly through the same in-repo VM path.
