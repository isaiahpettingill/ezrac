; Read an edited line with CP/M BDOS function 10, then return to CP/M.
; The first byte is the maximum input length; BDOS stores the entered length next.
; Build: ezrac build --target cpm-2.2-z80 --input-kind assembly line-input.asm

start:
    ld hl, input_buffer
    ex de, hl
    ld c, 0Ah
    call 0005h
    ld c, 00h
    call 0005h

input_buffer:
    db 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
    db 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
