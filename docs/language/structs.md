# Structs and field access

Structs group named fields:

```ezra
struct Point {
    x: u8
    y: u8
}

global origin: Point = Point { x: 0, y: 0 }
```

Read or write a field with `.`:

```ezra
fn move_right() {
    origin.x += 1
}
```

Take a field address for a pointer API:

```ezra
fn set_x(value: ptr<u8>) {
    *value = 10
}

fn update() {
    set_x(&origin.x)
}
```

Nested access paths and indexing can be combined where the types support them. Structs use target-defined field layout and alignment. They are aggregate storage, not supported multi-value function results; pass a struct by pointer when a function needs to mutate or return one.

Struct visibility follows ordinary top-level visibility. Mark the struct and any exported declarations `pub` when an imported module needs them.
