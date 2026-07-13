; Return cleanly to CP/M with BDOS function 0.
; Build: ezrac build --target cpm-2.2-z80 --input-kind assembly exit.asm

start:
    ld c, 00h
    call 0005h
