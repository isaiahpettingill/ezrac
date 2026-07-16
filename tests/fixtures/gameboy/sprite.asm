; Adapted from tbsp/simple-gb-asm-examples sprite.asm (CC0).
; Exercises OAM setup and post-increment loads used by typical GB programs.
rLCDC equ $FF40
rOBP0 equ $FF48
rOBP1 equ $FF49
LCDC_OBJ equ %00000010
LCDC_ENABLE equ %10000000
OAM_YFLIP equ %01000000
OAM_PAL1 equ %00010000
OAM_COUNT equ 40

entry:
    ld hl, tile_data
    ld de, $8000
    ld b, 16
copy_tile:
    ld a, (hl+)
    ld (de), a
    inc de
    dec b
    jr nz, copy_tile

    ld a, %11100100
    ldh (rOBP0), a
    ld a, %00011011
    ldh (rOBP1), a

    ld hl, $FE00
    ld a, 16
    ld (hl+), a
    sub 8
    ld (hl+), a
    xor a
    ld (hl+), a
    ld (hl+), a

    ld a, 19
    ld (hl+), a
    sub 6
    ld (hl+), a
    xor a
    ld (hl+), a
    ld a, OAM_YFLIP | OAM_PAL1
    ld (hl+), a

    xor a
    ld b, OAM_COUNT - 2
hide_unused:
    ld (hl), a
    inc l
    inc l
    inc l
    inc l
    dec b
    jr nz, hide_unused

    ld a, LCDC_ENABLE | LCDC_OBJ
    ldh (rLCDC), a
forever:
    jr forever

tile_data:
    db %00111100, %00111100
    db %01011110, %01000010
