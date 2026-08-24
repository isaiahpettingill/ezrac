# Pointers and pointer casts

Pointers use `ptr<T>`:

```ezra
global value: u8 = 3

fn increment(address: ptr<u8>) {
    *address += 1
}

fn main() {
    increment(&value)
}
```

Use `&` for address-of and `*` for dereference. Pointer arithmetic advances by the size of the pointed-to type as defined by the backend. A pointer cast is explicit:

```ezra
let bytes: ptr<u8> = cast<ptr<u8>>(&value)
let next: ptr<u8> = bytes + 1
```

## Arrays and pointers

Arrays do not decay to element pointers. Pass an array as storage with `ptr<[T; N]>`, or cast its address to `ptr<T>` when element-pointer arithmetic is intentional:

```ezra
global samples: [u8; 8] = [0, 0, 0, 0]

fn clear(samples: ptr<[u8; 8]>) {
    (*samples)[0] = 0
}

fn first_byte() -> u8 {
    let bytes: ptr<u8> = cast<ptr<u8>>(&samples)
    return *bytes
}
```

## Safety and hardware

The compiler checks types and address widths, not whether a runtime address is valid. Dereferencing an invalid address can corrupt memory or hardware state. MMIO needs [volatile declarations](globals.md#memory-mapped-io), and device-specific pointer use belongs in a target SDK wrapper where possible.

The target controls pointer width. Do not assume a pointer can hold an integer of the same source width on every backend; use the target's documented address width and explicit casts.
