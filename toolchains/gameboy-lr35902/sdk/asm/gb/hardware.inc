; EZRA Game Boy assembly SDK. Vendor this directory into assembly projects.
; Register addresses
rJOYP equ 0FF00h
rSB equ 0FF01h
rSC equ 0FF02h
rDIV equ 0FF04h
rTIMA equ 0FF05h
rTMA equ 0FF06h
rTAC equ 0FF07h
rIF equ 0FF0Fh
rNR10 equ 0FF10h
rNR11 equ 0FF11h
rNR12 equ 0FF12h
rNR13 equ 0FF13h
rNR14 equ 0FF14h
rNR21 equ 0FF16h
rNR22 equ 0FF17h
rNR23 equ 0FF18h
rNR24 equ 0FF19h
rNR30 equ 0FF1Ah
rNR31 equ 0FF1Bh
rNR32 equ 0FF1Ch
rNR33 equ 0FF1Dh
rNR34 equ 0FF1Eh
rNR41 equ 0FF20h
rNR42 equ 0FF21h
rNR43 equ 0FF22h
rNR44 equ 0FF23h
rNR50 equ 0FF24h
rNR51 equ 0FF25h
rNR52 equ 0FF26h
rLCDC equ 0FF40h
rSTAT equ 0FF41h
rSCY equ 0FF42h
rSCX equ 0FF43h
rLY equ 0FF44h
rLYC equ 0FF45h
rDMA equ 0FF46h
rBGP equ 0FF47h
rOBP0 equ 0FF48h
rOBP1 equ 0FF49h
rWY equ 0FF4Ah
rWX equ 0FF4Bh
rKEY1 equ 0FF4Dh
rVBK equ 0FF4Fh
rHDMA1 equ 0FF51h
rHDMA2 equ 0FF52h
rHDMA3 equ 0FF53h
rHDMA4 equ 0FF54h
rHDMA5 equ 0FF55h
rBGPI equ 0FF68h
rBGPD equ 0FF69h
rOBPI equ 0FF6Ah
rOBPD equ 0FF6Bh
rSVBK equ 0FF70h
rWAVE equ 0FF30h
rIE equ 0FFFFh

GB_TILE_DATA_0 equ 08000h
GB_TILE_DATA_1 equ 08800h
GB_BG_MAP_0 equ 09800h
GB_BG_MAP_1 equ 09C00h
GB_OAM equ 0FE00h
GB_OAM_COUNT equ 40
GB_OAM_ENTRY_SIZE equ 4
GB_OAM_Y equ 0
GB_OAM_X equ 1
GB_OAM_TILE equ 2
GB_OAM_FLAGS equ 3
OAMF_PRIORITY equ 80h
OAMF_YFLIP equ 40h
OAMF_XFLIP equ 20h
OAMF_DMG_PALETTE1 equ 10h
OAMF_CGB_BANK1 equ 08h

GB_PAD_RIGHT equ 01h
GB_PAD_LEFT equ 02h
GB_PAD_UP equ 04h
GB_PAD_DOWN equ 08h
GB_PAD_A equ 10h
GB_PAD_B equ 20h
GB_PAD_SELECT equ 40h
GB_PAD_START equ 80h

IEF_VBLANK equ 01h
IEF_STAT equ 02h
IEF_TIMER equ 04h
IEF_SERIAL equ 08h
IEF_JOYPAD equ 10h
LCDC_BG equ 01h
LCDC_OBJ equ 02h
LCDC_OBJ_8X16 equ 04h
LCDC_BG_MAP_1 equ 08h
LCDC_TILE_DATA_0 equ 10h
LCDC_WINDOW equ 20h
LCDC_WINDOW_MAP_1 equ 40h
LCDC_ENABLE equ 80h
STAT_MODE_MASK equ 03h
STAT_HBLANK equ 00h
STAT_VBLANK equ 01h
STAT_OAM equ 02h
STAT_TRANSFER equ 03h
SC_INTERNAL equ 01h
SC_CGB_FAST equ 02h
SC_START equ 80h
TAC_4096_HZ equ 00h
TAC_262144_HZ equ 01h
TAC_65536_HZ equ 02h
TAC_16384_HZ equ 03h
TAC_ENABLE equ 04h

GB_VBLANK_VECTOR equ 0040h
GB_STAT_VECTOR equ 0048h
GB_TIMER_VECTOR equ 0050h
GB_SERIAL_VECTOR equ 0058h
GB_JOYPAD_VECTOR equ 0060h

%macro GB_WRITE_IO(address, value)
    ld a, $value
    ldh ($address), a
%endmacro

%macro GB_READ_IO(address)
    ldh a, ($address)
%endmacro

%macro GB_INTERRUPTS_SET(mask)
    ld a, $mask
    ld (rIE), a
%endmacro

%macro GB_INTERRUPT_CLEAR(mask)
    ld a, $mask
    ldh (rIF), a
%endmacro

%macro GB_INTERRUPT_VECTOR(address, handler)
    org $address
    jp $handler
%endmacro

%macro GB_WAIT_VBLANK()
%%wait:
    ldh a, (rLY)
    cp 144
    jr c, %%wait
%endmacro

%macro GB_LCD_DISABLE_SAFE()
    %GB_WAIT_VBLANK
    ldh a, (rLCDC)
    and 7Fh
    ldh (rLCDC), a
%endmacro

%macro GB_LCD_ENABLE(flags)
    %GB_WRITE_IO rLCDC, $flags
%endmacro

; HL = ROM source, DE = VRAM destination, B = tile count. Clobbers AF, B, DE, HL.
%macro GB_TILE_UPLOAD()
%%tile:
    ld c, 16
%%byte:
    ld a, (hl+)
    ld (de), a
    inc de
    dec c
    jr nz, %%byte
    dec b
    jr nz, %%tile
%endmacro

; HL = map address, A = tile index. Fills one 32x32 hardware tile map.
%macro GB_TILEMAP_FILL()
    ld d, a
    ld bc, 0400h
%%loop:
    ld a, d
    ld (hl+), a
    dec bc
    ld a, b
    or c
    jr nz, %%loop
%endmacro

; HL = OAM entry, values are immediate screen coordinates/tile/flags.
%macro GB_SPRITE_SET(y, x, tile, flags)
    ld a, $y
    ld (hl+), a
    ld a, $x
    ld (hl+), a
    ld a, $tile
    ld (hl+), a
    ld a, $flags
    ld (hl), a
%endmacro

; HL = OAM entry.
%macro GB_SPRITE_HIDE()
    xor a
    ld (hl), a
%endmacro

; Expand this routine in HRAM and call it with A = source high byte.
; During DMG OAM DMA, code outside HRAM is not safely accessible.
; The caller owns interrupt state; the routine clobbers AF.
%macro GB_OAM_DMA_ROUTINE()
    ldh (rDMA), a
    ld a, 40
%%wait:
    dec a
    jr nz, %%wait
    ret
%endmacro

; Deprecated compatibility helper. This preserves fall-through behavior and is
; safe only when the entire expansion executes from HRAM.
%macro GB_OAM_DMA(source_high)
    ld a, $source_high
    ldh (rDMA), a
    ld a, 40
%%wait:
    dec a
    jr nz, %%wait
%endmacro

%macro GB_JOYPAD_SELECT_DPAD()
    %GB_WRITE_IO rJOYP, 20h
%endmacro

%macro GB_JOYPAD_SELECT_BUTTONS()
    %GB_WRITE_IO rJOYP, 10h
%endmacro

%macro GB_JOYPAD_DESELECT()
    %GB_WRITE_IO rJOYP, 30h
%endmacro

; Returns active-low low nibble in A. Read twice after selecting for settling.
%macro GB_JOYPAD_READ_DPAD()
    %GB_JOYPAD_SELECT_DPAD
    ldh a, (rJOYP)
    ldh a, (rJOYP)
    and 0Fh
%endmacro

%macro GB_JOYPAD_READ_BUTTONS()
    %GB_JOYPAD_SELECT_BUTTONS
    ldh a, (rJOYP)
    ldh a, (rJOYP)
    and 0Fh
%endmacro

%macro GB_TIMER_START(modulo, initial, clock)
    xor a
    ldh (rTAC), a
    ld a, $modulo
    ldh (rTMA), a
    ld a, $initial
    ldh (rTIMA), a
    ld a, $clock
    ldh (rTAC), a
%endmacro

%macro GB_SERIAL_START(value)
    ld a, $value
    ldh (rSB), a
    ld a, 81h
    ldh (rSC), a
%endmacro

%macro GB_SERIAL_WAIT()
%%wait:
    ldh a, (rSC)
    and 80h
    jr nz, %%wait
%endmacro

%macro GB_AUDIO_ENABLE()
    %GB_WRITE_IO rNR52, 80h
%endmacro

%macro GB_AUDIO_DISABLE()
    %GB_WRITE_IO rNR52, 00h
%endmacro

%macro GB_AUDIO_ROUTE(value)
    %GB_WRITE_IO rNR51, $value
%endmacro

%macro GB_AUDIO_VOLUME(value)
    %GB_WRITE_IO rNR50, $value
%endmacro

%macro GB_PULSE1(sweep, duty, envelope, low, high)
    %GB_WRITE_IO rNR10, $sweep
    %GB_WRITE_IO rNR11, $duty
    %GB_WRITE_IO rNR12, $envelope
    %GB_WRITE_IO rNR13, $low
    %GB_WRITE_IO rNR14, $high
%endmacro

%macro GB_PULSE2(duty, envelope, low, high)
    %GB_WRITE_IO rNR21, $duty
    %GB_WRITE_IO rNR22, $envelope
    %GB_WRITE_IO rNR23, $low
    %GB_WRITE_IO rNR24, $high
%endmacro

; HL points to 16 bytes of packed wave samples. Clobbers AF, B, DE, HL.
%macro GB_WAVE_LOAD()
    xor a
    ldh (rNR30), a
    ld de, rWAVE
    ld b, 16
%%loop:
    ld a, (hl+)
    ld (de), a
    inc de
    dec b
    jr nz, %%loop
%endmacro

%macro GB_WAVE_PLAY(length, level, low, high)
    %GB_WRITE_IO rNR30, 80h
    %GB_WRITE_IO rNR31, $length
    %GB_WRITE_IO rNR32, $level
    %GB_WRITE_IO rNR33, $low
    %GB_WRITE_IO rNR34, $high
%endmacro

%macro GB_NOISE(length, envelope, polynomial, control)
    %GB_WRITE_IO rNR41, $length
    %GB_WRITE_IO rNR42, $envelope
    %GB_WRITE_IO rNR43, $polynomial
    %GB_WRITE_IO rNR44, $control
%endmacro

; BC = byte count, DE = source, HL = destination. Clobbers AF, BC, DE, HL.
%macro GB_COPY_BYTES()
%%loop:
    ld a, (de)
    ld (hl+), a
    inc de
    dec bc
    ld a, b
    or c
    jr nz, %%loop
%endmacro

; BC = byte count, A = fill value, HL = destination. Clobbers BC, HL.
%macro GB_FILL_BYTES()
%%loop:
    ld (hl+), a
    dec bc
    ld a, b
    or c
    jr nz, %%loop
%endmacro

; Mapper primitives. Call from fixed ROM bank 0.
%macro GB_MBC_ENABLE_RAM()
    ld a, 0Ah
    ld (0000h), a
%endmacro

%macro GB_MBC_DISABLE_RAM()
    xor a
    ld (0000h), a
%endmacro

%macro GB_MBC1_ROM_BANK(bank)
    ld a, $bank
    ld (2000h), a
%endmacro

%macro GB_MBC3_ROM_BANK(bank)
    ld a, $bank
    ld (2000h), a
%endmacro

%macro GB_MBC3_SELECT_RAM_RTC(value)
    ld a, $value
    ld (4000h), a
%endmacro

%macro GB_MBC3_LATCH_RTC()
    xor a
    ld (6000h), a
    inc a
    ld (6000h), a
%endmacro

%macro GB_MBC5_ROM_BANK_LOW(bank)
    ld a, $bank
    ld (2000h), a
%endmacro

%macro GB_MBC5_ROM_BANK_HIGH(bit)
    ld a, $bit
    ld (3000h), a
%endmacro

%macro GB_MBC_RAM_BANK(bank)
    ld a, $bank
    ld (4000h), a
%endmacro
