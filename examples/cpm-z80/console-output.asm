; Print a $-terminated string with CP/M BDOS function 9, then return to CP/M.
; Build: ezrac build --target cpm-2.2-z80 --input-kind assembly console-output.asm

start:
    ld hl, message
    ex de, hl
    ld c, 09h
    call 0005h
    ld c, 00h
    call 0005h

message:
    db "Hello from EZRA on CP/M", 0Dh, 0Ah, "$"
