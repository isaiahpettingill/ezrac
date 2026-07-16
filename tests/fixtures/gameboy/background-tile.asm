; Adapted from tbsp/simple-gb-asm-examples background-tile.asm (CC0).
; Uses EZRA's documented parenthesized memory syntax.
rLY equ $FF44
rLCDC equ $FF40
rBGP equ $FF47
LCDC_BG equ %00000001
LCDC_TILE_DATA_0 equ %00010000
LCDC_ENABLE equ %10000000

entry:
    di
    ld sp, $E000
wait_vblank:
    ldh a, (rLY)
    cp 144
    jr c, wait_vblank
    xor a
    ldh (rLCDC), a

    ld hl, tile_data
    ld de, $8000
    ld b, 2 * 16
copy_tiles:
    ld a, (hl+)
    ld (de), a
    inc de
    dec b
    jr nz, copy_tiles

    ld hl, $9800
    ld (hl), 1
    inc hl
    ld bc, $9C00 - $9800 - 1
    ld d, 0
clear_map:
    ld (hl), d
    inc hl
    dec bc
    ld a, b
    or c
    jr nz, clear_map

    ld a, LCDC_ENABLE | LCDC_TILE_DATA_0 | LCDC_BG
    ldh (rLCDC), a
forever:
    jr forever

tile_data:
    db %00000000, %11111111
    db %01000010, %10000001
