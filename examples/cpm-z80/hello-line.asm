; Print "EZRA" and CR/LF through CP/M BDOS function 2.
; Build: ezra assemble --target cpm-2.2-z80 hello-line.asm

start:
    ld c, 02h
    ld e, 45h
    call 0005h
    ld e, 5Ah
    call 0005h
    ld e, 52h
    call 0005h
    ld e, 41h
    call 0005h
    ld e, 0Dh
    call 0005h
    ld e, 0Ah
    call 0005h
    ld c, 00h
    call 0005h
