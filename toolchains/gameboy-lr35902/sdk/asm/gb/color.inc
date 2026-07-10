include "hardware.inc"

; CGB-only helpers. Use with gameboy-color-lr35902.
%macro GBC_VRAM_BANK(bank)
    %GB_WRITE_IO rVBK, $bank
%endmacro

%macro GBC_WRAM_BANK(bank)
    %GB_WRITE_IO rSVBK, $bank
%endmacro

%macro GBC_BG_PALETTE_INDEX(index)
    %GB_WRITE_IO rBGPI, $index
%endmacro

%macro GBC_BG_PALETTE_DATA(value)
    %GB_WRITE_IO rBGPD, $value
%endmacro

%macro GBC_OBJ_PALETTE_INDEX(index)
    %GB_WRITE_IO rOBPI, $index
%endmacro

%macro GBC_OBJ_PALETTE_DATA(value)
    %GB_WRITE_IO rOBPD, $value
%endmacro

%macro GBC_HDMA(source, destination, blocks)
    %GB_WRITE_IO rHDMA1, $source
    %GB_WRITE_IO rHDMA2, 00h
    %GB_WRITE_IO rHDMA3, $destination
    %GB_WRITE_IO rHDMA4, 00h
    %GB_WRITE_IO rHDMA5, $blocks
%endmacro

%macro GBC_HDMA_CANCEL()
    ldh a, (rHDMA5)
    and 7Fh
    ldh (rHDMA5), a
%endmacro

%macro GBC_BG_COLOR_LOW(index, value)
    ld a, $index
    or 80h
    ldh (rBGPI), a
    ld a, $value
    ldh (rBGPD), a
%endmacro

%macro GBC_OBJ_COLOR_LOW(index, value)
    ld a, $index
    or 80h
    ldh (rOBPI), a
    ld a, $value
    ldh (rOBPD), a
%endmacro

%macro GBC_PREPARE_DOUBLE_SPEED()
    %GB_WRITE_IO rKEY1, 01h
    stop
%endmacro
