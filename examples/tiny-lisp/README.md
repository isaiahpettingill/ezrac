# TinyLisp: conditional-compilation example

`TinyLisp` is a small interactive LISP dialect implemented as one EZRA source
file. It demonstrates target-specific imports and terminal glue with
`@cfg(target(...))`, while its reader, recursive evaluator, number printer, and
REPL are shared between all supported systems.

The language is intentionally small:

```lisp
(+ 1 2)
(* (+ 2 3) 4)
(- 100 58)
(/ 144 12)
```

Each form has one binary operator and exactly two expressions. Expressions are
unsigned 16-bit decimal integers or nested forms. Arithmetic wraps on overflow;
division by zero and malformed input report `error`. Press `Q` at a prompt to
leave the REPL (the TI cartridge then idles in its exit loop).

## Build

Run these commands from the repository root:

```sh
# Agon Light MOS executable (.bin)
cargo run -- build examples/tiny-lisp/main.ezra

# CP/M 2.2 command (.com)
cargo run -- build --target cpm-2.2-z80 examples/tiny-lisp/main.ezra

# Commodore 64 program (.prg); the 6502 backend is an optional feature
cargo run --features mos6502 -- build --target commodore64-6502 examples/tiny-lisp/main.ezra

# ZX Spectrum tape image (.tap)
cargo run -- build --target zxspectrum-z80 examples/tiny-lisp/main.ezra

# TI-99/4A cartridge image (.bin); the TMS9900 backend is an optional feature
cargo run --features tms9900 -- build --target ti99-4a-tms9900 examples/tiny-lisp/main.ezra
```

Artifacts are written beneath `examples/tiny-lisp/target/<target>/`.

## Platform layer

The five `@cfg(target(...))` branches select only their relevant SDK modules:

| Target | Console implementation |
| --- | --- |
| `agonlight-mos-ez80` | `agon.console`, with explicit echo for MOS key reads |
| `cpm-2.2-z80` | `cpm.console` BDOS input/output |
| `commodore64-6502` | `c64.kernal` keyboard and output, after mapping KERNAL ROM/I/O |
| `zxspectrum-z80` | `zx.keyboard` translated blocking input and `zx.rom` character output; Caps Shift and Symbol Shift mappings are handled by the Spectrum ROM |
| `ti99-4a-tms9900` | `ti99.input` console-ROM KSCAN input and `ti99.vdp` 32-column name-table output; KSCAN temporarily switches to the GPL workspace at `>83E0` and restores Ezra's `>8300` workspace |

The TI target is a standard one-bank cartridge and assumes the target profile's
conventional 32 KiB expansion RAM. Its VDP setup uses the console character
patterns already present when the cartridge menu launches the program.

No target-specific code appears in the parser or evaluator. This keeps the
example focused on Ezra's declaration-level conditional compilation rather
than on a shared lowest-level ABI.
