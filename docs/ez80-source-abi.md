# eZ80/Z80-family source ABI

EZRAC lowers EZRA source for the eZ80 (ADL mode), Z80, R800, Z80N, and Z180 through the shared Z80-family backend. The standalone assembler coverage lives in [eZ80/Z80 opcode coverage](ez80-opcode-coverage.md); this page documents the calling convention the generated code uses.

## EZRA source-function ABI

Non-naked EZRA functions use a calculated IX stack frame:

- SP is the descending stack pointer. IX is the frame pointer; IY is compiler-owned scratch.
- The prologue is `push ix`, `ld ix, 000000h`, `add ix, sp`, then a reservation of the frame bytes below SP. The epilogue restores SP with `ld sp, ix` and pops IX, so every invocation unwinds exactly what it reserved regardless of frame size.
- The first three parameters travel in registers: argument 1 in HL, argument 2 in DE (B for u8), and argument 3 in BC (C for u8). When the second parameter is one byte and the third is wider, registers cannot hold both; those parameters are passed on the stack instead of in registers.
- Parameters from four onward, and any register-blocked combination, are pushed as stack arguments. Each occupies its natural scalar width: u8/bool values take one byte and pointers/wide values take their full byte width. Stack arguments sit above the saved IX and return address, read as positive `(ix+d)` displacements.
- Locals that need memory, allocator spills, aggregate storage, compiler scratch, and call temporaries live inside the active invocation's frame at fixed negative IX offsets. Address-taken locals keep stable frame offsets for the whole invocation. Wide accesses beyond the signed 8-bit displacement range stage through in-frame relay scratch.
- A caller reserves and fills the argument area, calls the callee, and releases the area after the return. No parameter, local, spill, result, or call snapshot uses fixed RAM: direct recursion, mutual recursion, and function-pointer re-entry are supported without static snapshots.
- Scalar results return in HL (A for u8). Two-result calls pass a hidden return pointer as the first stack word and return the first value in HL/A and the second in BC or memory through that pointer.
- Approved tail calls unwind the caller frame with `ld sp, ix` / `pop ix` before `jp` into the callee, so tail-recursion runs in constant stack space.
- Globals, MMIO ports, volatile regions, strings, embeds, and SDK data retain their fixed addresses. Inline assembly `mem` operands for automatic locals expand to frame-relative operands such as `(ix-6)`; bare use of `ix` inside inline asm still requires declaring the `ix` clobber.
- Naked functions receive no prologue, epilogue, or parameter marshalling. Interrupt functions push AF/BC/DE/HL/IX/IY themselves, build the same style of frame anchored on the saved-IY cell, and return with `reti`.
- The Intel 8080 family (i8080/i8085) keeps the legacy static RAM convention with recursive-call snapshots because those CPUs have no IX addressing.
