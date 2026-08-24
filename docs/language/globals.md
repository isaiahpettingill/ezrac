# Globals, ports, and MMIO

## Globals

`global` allocates mutable storage in the program data area:

```ezra
global score: u16 = 0
global buffer: [u8; 16] = [0, 0, 0, 0]

fn add_point() {
    score += 1
}
```

Add `pub` when another module needs the declaration. Globals are runtime storage; they are not compile-time constants.

## Ports

A `port` names an I/O port. Use `out` to write and `in` to read:

```ezra
port DEBUG: u8 = 0x0C

fn main() {
    out DEBUG, 'A'
    let status: u8 = in DEBUG
}
```

Port width and instruction lowering depend on the target. Prefer the target SDK when a platform has a named device API.

## Memory-mapped I/O

`mmio` names a memory-mapped address. Add `volatile` when reads or writes have device side effects or must not be treated as ordinary memory:

```ezra
volatile mmio FRAMEBUFFER: ptr<u8> = 0x080000u24

fn clear() {
    *FRAMEBUFFER = 0
}
```

Use `volatile` for the declaration, not as a replacement for understanding the device's access protocol. The `ezra.mem` block operations require ordinary nonvolatile memory; use `peek8`/`poke8` or an SDK operation for explicit device byte accesses where supported.

## Address and initialization rules

Global initializers and embedded data are placed through the selected target layout. Check `.map` and `.size` after a build when a target has tight RAM or ROM ranges. [Targets and layouts](../targets-and-layouts.md) describes section placement.
