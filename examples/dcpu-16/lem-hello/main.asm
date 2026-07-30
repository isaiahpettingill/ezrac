; LEM1802 text output through the DCPU Standard Machine macro SDK.
include "../../../toolchains/generic-dcpu-bare/sdk/asm/dcpu.inc"

start:
    %lem_set_border 1
    %keyboard_clear
    %clock_set_rate 60
    %speaker_set 0, 0
    %speaker_set 1, 0
    %lem_put_cell 0, 0xf045 ; E
    %lem_put_cell 1, 0xf05a ; Z
    %lem_put_cell 2, 0xf052 ; R
    %lem_put_cell 3, 0xf041 ; A
    %lem_put_cell 4, 0xf020 ; space
    %lem_put_cell 5, 0xf044 ; D
    %lem_put_cell 6, 0xf043 ; C
    %lem_put_cell 7, 0xf050 ; P
    %lem_put_cell 8, 0xf055 ; U

counter:
    add [0x8020], 1
    set pc, counter
