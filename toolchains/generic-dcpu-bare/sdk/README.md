# DCPU-16 SDK

`dcpu.*` modules provide typed constants for the DCPU-16 Standard Machine in
dcpu-16-libretro. Import them from an EZRA source project targeting
`generic-dcpu-bare`.

The DCPU source backend currently supports operand-free inline assembly only.
Use `toolchains/generic-dcpu-bare/sdk/asm/dcpu.inc` for reusable device-command
macros in handwritten assembly. Vendor that file into a project and include it
with a relative path.

The macro SDK covers device slots, generic `HWI`, LEM1802 setup and text cells,
keyboard reads, clock setup, and two-channel speaker frequencies. Other device
commands can use `%dcpu_hwi(device)` after placing the documented command and
arguments in registers.
