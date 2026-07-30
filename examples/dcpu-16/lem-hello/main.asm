; LEM1802 is mapped at word address 0x8000 by the libretro core's
; Standard Compatibility profile. Each word is ffff bbbb B ccccccc:
; blink, background, foreground, character.

start:
    set i, 0
    set [0x8000+i], 0xf045 ; E
    add i, 1
    set [0x8000+i], 0xf05a ; Z
    add i, 1
    set [0x8000+i], 0xf052 ; R
    add i, 1
    set [0x8000+i], 0xf041 ; A
    add i, 1
    set [0x8000+i], 0xf020 ; space
    add i, 1
    set [0x8000+i], 0xf044 ; D
    add i, 1
    set [0x8000+i], 0xf043 ; C
    add i, 1
    set [0x8000+i], 0xf050 ; P
    add i, 1
    set [0x8000+i], 0xf055 ; U

counter:
    add [0x8020], 1
    set pc, counter
