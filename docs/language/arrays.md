# Arrays and indexing

Fixed-size arrays use `[T; LENGTH]`:

```ezra
const START: [u8; 4] = [1, 2, 3, 4]
global screen: [u8; 16] = [0, 0, 0, 0]

fn set_first() {
    screen[0] = 0xFF
}
```

Array length is part of the type. Indexing reads or writes one element. Nested arrays are allowed where the target backend supports their storage shape.

## Passing arrays

Arrays are storage, not scalar values. Pass them through a pointer parameter:

```ezra
fn clear(screen: ptr<[u8; 16]>) {
    (*screen)[0] = 0
}

fn main() {
    clear(&screen)
}
```

An array does not implicitly become `ptr<T>`. If you need byte-wise traversal, explicitly cast its address:

```ezra
let cursor: ptr<u8> = cast<ptr<u8>>(&screen)
*(cursor + 1) = 7
```

## Initializers and constants

Constant arrays can use fewer values than their declared length; missing values are zero-filled. `zeroes()` is accepted for fixed-size array initialization:

```ezra
const FOUR_ZEROES: [u8; 4] = zeroes()
const TABLE: [u8; 3] = [4, 7, 9]
```

Array values are not supported as function multi-results. Return or mutate them through pointers.
