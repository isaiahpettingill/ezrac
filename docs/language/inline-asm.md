# Inline assembly

Inline assembly is a statement. Each instruction line is a string literal:

```ezra
asm {
    "nop"
}
```

Use `volatile` for assembly with side effects that must not be removed or treated as an ordinary expression:

```ezra
asm volatile {
    "ei"
}
```

## Operands and clobbers

Operands document values passed into or out of the assembly and registers or state changed by it:

```ezra
asm volatile (
    in value: u8 as reg8,
    out result: u8 as reg8,
    clobber af,
) {
    "ld a, 1"
}
```

Operand classes are `reg8`, `reg16`, `reg24`, `mem`, and `imm`. Clobbers can name registers and machine state such as `flags`, `memory`, and `ports` where the target supports them.

The source syntax is shared, but register names, instructions, ABI rules, clobbers, and operand widths are target-specific. The lowering is intentionally simple. Use a target SDK function for reusable hardware access; use inline assembly for a small target-specific operation with a clear contract.

For an entire handwritten assembly source file, use [`ezrac assemble`](../assembly.md) instead. `extern asm fn` declarations connect source calls to assembly routines but do not generate those routines.
