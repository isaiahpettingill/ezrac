# Conditional compilation and banking syntax

Put `@cfg(...)` before a top-level declaration to include it only for matching target or compiler conditions:

```ezra
@cfg(cpu("ez80"))
pub const POINTER_BYTES: u8 = 3

@cfg(any(cpu("z80"), cpu("z180")))
pub const POINTER_BYTES: u8 = 2
```

Supported predicates include:

```text
target("full-target-triple")
target_family("first-target-part")
cpu("ez80" | "z80" | "r800" | "z80n" | "z180" | "i8080" | "i8085" | "lr35902")
vendor("target-component")
os("mos" | "cpm" | "baremetal")
pointer_width(16 | 24)
address_width(16 | 24)
feature("target-component")
debug
release
all(...)
any(...)
not(...)
```

`feature("...")` matches a target-triple component other than the CPU. Unknown feature names are rejected rather than treated as false. Target conditions are evaluated before source lowering, so excluded declarations do not need to compile for the current target.

## Banking syntax

`@cfg(bank(N))` is separate from conditional compilation. It records a bank-placement attribute:

```ezra
@cfg(bank(3))
pub fn load_level() {}
```

Pointers can carry an explicit bank postfix:

```ezra
let tiles: ptr<u8> = tile_data@3
let next: ptr<u8> = (tiles + 16)@3
```

Enable the project syntax/configuration foundation with:

```toml
[banking]
enabled = true
```

Generic `@cfg(bank(N))` metadata and pointer-bank syntax do not by themselves implement bank switching, pointer representation, linking, or runtime behavior. The Game Boy targets have an implemented MBC banking path; other targets may only preserve the metadata. Treat banking behavior as target-owned and check the relevant target guide, such as the [Game Boy assembly guide](../gameboy-assembly.md).
