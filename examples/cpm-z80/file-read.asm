; Open README.TXT, read its first 128-byte record into the default DMA buffer,
; close it, and return to CP/M. BDOS returns 00h on success and FFh on error.
; Build: ezrac build --target cpm-2.2-z80 --input-kind assembly file-read.asm

start:
    ld hl, file_control_block
    ex de, hl
    ld c, 0Fh
    call 0005h
    cp 00h
    jp nz, done

    ld hl, 0080h
    ex de, hl
    ld c, 1Ah
    call 0005h

    ld hl, file_control_block
    ex de, hl
    ld c, 14h
    call 0005h

    ld hl, file_control_block
    ex de, hl
    ld c, 10h
    call 0005h

done:
    ld c, 00h
    call 0005h

file_control_block:
    db 0, "README  TXT", 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
    db 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
