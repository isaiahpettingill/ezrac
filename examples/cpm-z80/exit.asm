; Minimal CP/M .COM program.
; Build: ezra assemble --target cpm-2.2-z80 exit.asm

start:
    ld c, 00h
    call 0005h
