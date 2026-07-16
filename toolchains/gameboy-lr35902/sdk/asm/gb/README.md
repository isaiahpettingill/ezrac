# EZRA Game Boy Assembly SDK

Vendor this `gb` directory into an assembly project and include
`hardware.inc`; CGB projects may include `color.inc` instead. The files expose
hardware register constants and macros for interrupts, LCD access, OAM DMA,
joypads, timers, serial, audio, byte copying/filling, MBC1/MBC3/MBC5 banking,
and CGB VRAM/WRAM banks, palettes, HDMA, and speed switching.

Macros document their register use in comments where stateful. Hardware timing
still matters: VRAM and palette writes are unavailable during PPU mode 3, OAM
DMA must execute from HRAM on real hardware, LCD disable should happen during
VBlank, and bank switches must execute from fixed ROM. Expand
`GB_OAM_DMA_ROUTINE` into HRAM and call it with the source page in A. The older
`GB_OAM_DMA` fall-through macro is retained for compatibility but is safe only
when its entire expansion is in HRAM. These helpers do not hide the other
hardware constraints.

References: Pan Docs (`https://gbdev.io/pandocs/`), the complete SM83 opcode
table (`https://gbdev.io/gb-opcodes/optables/`), the GB ASM Tutorial
(`https://gbdev.io/gb-asm-tutorial/`), and the CC0 simple Game Boy assembly
examples (`https://github.com/tbsp/simple-gb-asm-examples`).
