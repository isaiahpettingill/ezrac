# R800 assembly and code generation

EZRAC supports the R800 as a 16-bit Z80-family CPU. Use `r800` for standalone assembly or `bare-r800` for a raw binary target:

```sh
cargo run -- assemble --cpu r800 program.asm
cargo run -- build --target bare-r800 program.ezra
```

The R800 target uses the Z80 register and 16-bit address model. `IXH`, `IXL`, `IYH`, and `IYL` are accepted as supported R800 register names through the shared Z80 instruction encoder.

## Multiply instructions

All documented R800 multiply forms are supported:

```asm
mulub a, b
mulub a, c
mulub a, d
mulub a, e
mulub a, h
mulub a, l
mulub a, a

muluw hl, bc
muluw hl, de
muluw hl, hl
muluw hl, sp
```

`MULUB A,r` multiplies two unsigned 8-bit values and writes the 16-bit result to `HL`. `MULUW HL,rr` multiplies two unsigned 16-bit values and writes the 32-bit result to `DE:HL`.

EZRA source multiplication uses these instructions on the R800 target. An 8-bit result keeps the low byte from `L`; a 16-bit result keeps the low word in `HL`, matching the language's wrapping integer rules.

## Execution

The built-in test runner uses the `ez80` crate's R800 mode. It executes R800 multiply opcodes and the shared Z80 instruction set with 16-bit `PC`, `SP`, pointers, and addresses.

The R800's internal DMA, mapper, interrupt, and DRAM control registers are chip interfaces, not extra instruction operands or allocator-visible CPU registers. They are not modeled as general-purpose registers by the compiler.

References:

- [R800 User's Manual](https://map.grauw.nl/resources/cpu/r800_users_manual.php)
- [Nestor80](https://github.com/Konamiman/Nestor80)
