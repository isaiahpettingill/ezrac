; Minimal SNES LoROM entry code. EZRA's SNES packager adds the internal
; header, checksum, and native/emulation vectors.

reset:
    sei
    cld
    clc
    xce
    rep #$10
    ldx #$1FFF
    txs
    sep #$10
    rep #$20
    lda #$0000
    tcd
    sep #$20
    lda #$00
    pha
    plb

    lda #$80
    sta $2100          ; force blank
    stz $4200          ; disable NMI and auto joypad read
    stz $420B          ; disable DMA
    stz $420C          ; disable HDMA

    lda #$00
    sta $2121          ; CGRAM entry 0
    lda #$00
    sta $2122          ; blue color, low byte
    lda #$7C
    sta $2122          ; blue color, high byte

    lda #$0F
    sta $2100          ; display on, full brightness

forever:
    lda $4212
    and #$80
    beq forever
    bra forever
