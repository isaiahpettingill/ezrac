# Intel 8086 assembler mode

EZRAC provides an optional, strict Intel 8086 standalone assembler. Enable the `i8086` Cargo feature and select the `bare-i8086` target:

```sh
cargo run --features i8086 -- assemble \
  --cpu i8086 --target bare-i8086 --base 100h \
  -o program.bin program.asm
```

`8086` is accepted as a CPU alias. Files ending in `.i8086` or `.8086` are detected as assembly input in addition to `.asm` and `.s`.

This target is assembly-only. It does not yet provide EZRA source lowering, an ABI/runtime, an emulator test backend, DOS `.COM`/MZ packaging, or 80186/80286 profiles. Those remain separate work under the wider Intel 8086-family and MS-DOS issues.

## Target and output model

The 8086 hardware has a 20-bit physical address bus, but the initial `bare-i8086` profile deliberately exposes one 16-bit, 64 KiB segment and emits a raw binary for that segment. Labels, `org`, near branches, and ordinary memory offsets are therefore 16-bit segment offsets. Far calls and jumps can still encode explicit `segment:offset` pointers.

Words, immediates, displacements, data emitted with `dw`, and far-pointer fields are little-endian. In a far pointer the offset word is emitted before the segment word.

## Source syntax

The assembler uses Intel operand order and bracketed memory syntax:

```asm
org 100h

start:
    mov ax, data
    mov ds, ax
    mov si, message
    mov cx, message_end - message
    rep movsb
    int 20h

message:
    db "Hello"
message_end:
```

Shared assembler facilities provide labels, case-insensitive symbol references, equates, `org`, sections, `db`/`byte`, `dw`/`word`, includes, conditionals, and hygienic macros. Numeric expressions support decimal, `0x`/`$`/`>`/trailing-`h` hexadecimal, `0b`/`%` binary, `0o` octal, arithmetic, and bitwise operators. `$` by itself is the current instruction address.

Use `BYTE PTR` or `WORD PTR` when a memory operand's size cannot be inferred:

```asm
inc byte ptr [bx]
mov word ptr [result], 1234h
not word ptr es:[di]
```

The accepted 16-bit effective addresses are:

```text
BX+SI  BX+DI  BP+SI  BP+DI  SI  DI  BP  BX  disp16
```

Each register form may include one constant or symbolic displacement. Term order is normalized, signed 8-bit displacements are selected when an absolute expression fits, `[BP]` emits the required zero displacement, and unresolved symbolic displacements use a stable 16-bit form. Scaled indexes, `SP` addressing, 32-bit registers, and all non-8086 effective-address combinations are rejected.

Segment overrides use either operand syntax or a leading prefix:

```asm
mov ax, es:[di]
cs: mov ax, [bx]
rep ds: movsb
```

`ES`, `CS`, `SS`, and `DS` are supported. Overrides are rejected on instructions without an overridable memory access, including `LEA`, fixed-`ES` `STOS`/`SCAS`, branches, and register-only forms.

## Branch and immediate sizing

The two assembly passes always agree on instruction size:

- `JMP label` defaults to the near `rel16` form.
- `JMP SHORT label` forces `rel8`; `JMP NEAR label` forces `rel16`.
- `CALL label` is near unless an explicit far immediate or `FAR PTR` memory operand is used.
- `Jcc`, `LOOP`, `LOOPE`/`LOOPZ`, `LOOPNE`/`LOOPNZ`, and `JCXZ` are always short and diagnose an out-of-range target.
- Symbolic effective-address displacements and symbolic word ALU immediates retain their 16-bit encodings. Absolute word immediates use opcode `83` only when byte sign extension preserves the requested word value.
- Literal `INT 3` uses opcode `CC`; symbolic vector expressions retain the stable two-byte `CD 03` form.

## Instruction coverage

The encoder covers every documented 8086 integer instruction and native form:

- `MOV`, `XCHG`, `PUSH`, `POP`, `PUSHF`, `POPF`, `LEA`, `LDS`, `LES`, and `XLAT`;
- `ADD`, `ADC`, `SUB`, `SBB`, `CMP`, `INC`, `DEC`, `NEG`, `MUL`, `IMUL`, `DIV`, `IDIV`, `AAA`, `AAS`, `DAA`, `DAS`, `AAM`, `AAD`, `CBW`, and `CWD`;
- `AND`, `OR`, `XOR`, `NOT`, and `TEST`;
- `ROL`, `ROR`, `RCL`, `RCR`, `SHL`/`SAL`, `SHR`, and `SAR`, with counts of `1` or `CL`;
- near, short, far, direct, and indirect `CALL`/`JMP` forms; every short conditional jump and documented alias; `LOOP*`, `JCXZ`, `RET`/`RETN`, `RETF`, and `IRET`;
- `INT`, `INT3`, `INTO`, `IN`, and `OUT`;
- `MOVSB/W`, `CMPSB/W`, `STOSB/W`, `LODSB/W`, and `SCASB/W` with legal `REP`, `REPE`/`REPZ`, and `REPNE`/`REPNZ` combinations;
- `CLC`, `STC`, `CMC`, `CLD`, `STD`, `CLI`, `STI`, `LAHF`, `SAHF`, `HLT`, `WAIT`/`FWAIT`, and `NOP`;
- raw `ESC 0..63, memory` and `ESC 0..63, 0..7` coprocessor-interface encodings.

`LOCK` is accepted only for documented read-modify-write operations whose encoded destination is memory. A memory `XCHG` remains implicitly locked and may also carry an explicit, redundant `LOCK` prefix.

## Strict 8086 profile

The mode intentionally rejects undocumented opcode aliases and later-family additions. This includes `POP CS`, `SALC`, opcode `82`, reserved ModR/M group extensions, immediate shift counts other than `1`, immediate `PUSH`, multi-operand `IMUL`, `PUSHA`/`POPA`, `BOUND`, string port I/O, `ENTER`/`LEAVE`, protected-mode instructions, near `Jcc`, `FS`/`GS`, operand/address-size prefixes, 32-bit registers, and every 80386+ instruction.

The tests include exact golden encodings across every instruction category, all eight 16-bit ModR/M effective-address selectors, displacement and immediate boundaries, aliases, prefixes, far pointers, labels/fixups, stable two-pass sizing, and explicit rejection of post-8086 forms.
