; DCPU-16 Standard Machine helpers for dcpu-16-libretro.
; Vendor this file into an assembly project and include it with a relative path.
; The Standard Compatibility device order is fixed by the core.

%define DCPU_LEM1802 0
%define DCPU_KEYBOARD 1
%define DCPU_CLOCK 2
%define DCPU_M35FD 3
%define DCPU_SPED3 4
%define DCPU_SPEAKER 5
%define DCPU_RETROPAD_1 6

%define DCPU_LEM_SCREEN 0
%define DCPU_LEM_FONT 1
%define DCPU_LEM_PALETTE 2
%define DCPU_LEM_BORDER 3

%define DCPU_KEYBOARD_CLEAR 0
%define DCPU_KEYBOARD_READ 1
%define DCPU_KEYBOARD_IS_DOWN 2
%define DCPU_KEYBOARD_INTERRUPT 3

%define DCPU_CLOCK_RATE 0
%define DCPU_CLOCK_TICKS 1
%define DCPU_CLOCK_INTERRUPT 2

; Run an HWI command already prepared in A/B/C/X/Y/Z.
%macro dcpu_hwi(device)
    hwi $device
%endmacro

; Map LEM1802 text memory at a DCPU word address.
%macro lem_map_screen(address)
    set a, ${DCPU_LEM_SCREEN}
    set b, $address
    hwi ${DCPU_LEM1802}
%endmacro

; Set LEM1802's border color, from 0 through 15.
%macro lem_set_border(color)
    set a, ${DCPU_LEM_BORDER}
    set b, $color
    hwi ${DCPU_LEM1802}
%endmacro

; Write one LEM1802 text cell. `cell` is 0 through 383. `word` is the fully
; encoded cell word: blink/background/foreground/ASCII.
%macro lem_put_cell(cell, word)
    set [0x8000 + $cell], $word
%endmacro

; Set Generic Clock ticks per second. Zero disables it.
%macro clock_set_rate(ticks_per_second)
    set a, ${DCPU_CLOCK_RATE}
    set b, $ticks_per_second
    hwi ${DCPU_CLOCK}
%endmacro

; Clear pending key presses.
%macro keyboard_clear()
    set a, ${DCPU_KEYBOARD_CLEAR}
    hwi ${DCPU_KEYBOARD}
%endmacro

; Read one queued key code into C. Zero means no key is pending.
%macro keyboard_read()
    set a, ${DCPU_KEYBOARD_READ}
    hwi ${DCPU_KEYBOARD}
%endmacro

; Set a speaker channel frequency in Hz. Channel zero is left; one is right.
%macro speaker_set(channel, frequency)
    set a, $channel
    set b, $frequency
    hwi ${DCPU_SPEAKER}
%endmacro
