; A DCPU-16 Standard Machine program using EZRAC's vendorable macro SDK.
include "../../../toolchains/generic-dcpu-bare/sdk/asm/dcpu.inc"

start:
    ; Explicitly use the Standard Compatibility display mapping.
    %lem_map_screen 0x8000
    %lem_set_border 9

    ; Start clean and request a 60 Hz device tick.
    %keyboard_clear
    %clock_set_rate 60

    ; Start a quiet stereo tone. Set both channels to zero to silence it again.
    %speaker_set 0, 440
    %speaker_set 1, 660

    ; Write "SDK" with white foreground on black background.
    %lem_put_cell 0, 0xf053 ; S
    %lem_put_cell 1, 0xf044 ; D
    %lem_put_cell 2, 0xf04b ; K

loop:
    ; C receives a queued key, or zero if none is pending.
    %keyboard_read
    ife c, 0
        set pc, loop

    ; Any key changes the border, then returns to polling.
    add [0x8000], 0x0100
    set pc, loop
