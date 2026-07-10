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
rIE equ 0FFFFh

IEF_VBLANK equ 01h
IEF_STAT equ 02h
IEF_TIMER equ 04h
IEF_SERIAL equ 08h
IEF_JOYPAD equ 10h
LCDC_BG equ 01h
LCDC_OBJ equ 02h
LCDC_OBJ_8X16 equ 04h
LCDC_WINDOW equ 20h
LCDC_ENABLE equ 80h
SC_INTERNAL equ 01h
SC_CGB_FAST equ 02h
SC_START equ 80h
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

%macro GB_OAM_DMA(source_high)
    di
    ld a, $source_high
    ldh (rDMA), a
    ld a, 40
%%wait:
    dec a
    jr nz, %%wait
    ei
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
