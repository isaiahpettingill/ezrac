; Print one character through CP/M BDOS function 2.
; Build: ezra assemble --target cpm-2.2-z80 hello-char.asm

start:
    ld c, 02h
    ld e, 48h
    call 0005h
    ld c, 00h
    call 0005h
