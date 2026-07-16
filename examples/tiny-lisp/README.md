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
return to the host.

## Build

Run these commands from the repository root:

```sh
# Agon Light MOS executable (.bin)
cargo run -- build examples/tiny-lisp/main.ezra

# CP/M 2.2 command (.com)
cargo run -- build --target cpm-2.2-z80 examples/tiny-lisp/main.ezra

# Commodore 64 program (.prg); the 6502 backend is an optional feature
cargo run --features mos6502 -- build --target commodore64-6502 examples/tiny-lisp/main.ezra
```

Artifacts are written beneath `examples/tiny-lisp/target/<target>/`.

## Platform layer

The three `@cfg(target(...))` branches select only their relevant SDK modules:

| Target | Console implementation |
| --- | --- |
| `agonlight-mos-ez80` | `agon.console`, with explicit echo for MOS key reads |
| `cpm-2.2-z80` | `cpm.console` BDOS input/output |
| `commodore64-6502` | `c64.kernal` keyboard and output, after mapping KERNAL ROM/I/O |

No target-specific code appears in the parser or evaluator. This keeps the
example focused on Ezra's declaration-level conditional compilation rather
than on a shared lowest-level ABI.
