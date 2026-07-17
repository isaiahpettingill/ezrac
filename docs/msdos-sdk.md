# MS-DOS `.COM` SDK for Intel 8086

The `msdos-com-i8086` target builds EZRA source into a flat MS-DOS `.COM`
program using the optional `i8086` backend:

```sh
cargo run --features i8086 -- build --target msdos-com-i8086 program.ezra
```

The build writes generated assembly, a symbol map, and `program.com` beneath the
project's `target/msdos-com-i8086/` directory. MZ `.EXE` relocation and header
packaging are intentionally deferred; this target produces `.COM` files only.

## Load and startup model

DOS constructs a Program Segment Prefix (PSP) at offsets `0000h..00FFh` and
loads the first byte of the `.COM` image at offset `0100h`. Execution starts at
`0100h` with `CS`, `DS`, `ES`, and `SS` referring to the program's allocation.
The EZRA startup stub:

1. copies `CS` to `DS` and `ES`;
2. checks the PSP end-segment field and exits with status `FFh` if DOS granted
   fewer than `0F80h` paragraphs (62 KiB);
3. preserves DOS's loader-provided `SS:SP` exactly;
4. clears the direction flag;
5. initializes compiler-owned static storage;
6. calls `main` with near-call semantics; and
7. terminates a returning `main` with `AX=4C00h`, `INT 21h`.

The allocation check protects the fixed regions through `EFFFh` and leaves at
least 2 KiB above them for DOS's loader stack. Preserving that stack is still
important: forcing `SP=FFFEh` can place it outside a partial allocation. DOS
normally grants `.COM` programs the largest available block, but unusually
constrained launches can fail this check before `main`. Application code should
normally return from `main` rather than calling a termination service directly.

## Memory and pointer rules

The source ABI is a single-segment, 16-bit near-pointer model. Ordinary
`ptr<T>` values are offsets relative to `DS`; they are not 20-bit physical
addresses and cannot represent arbitrary `segment:offset` pairs.

Most `dos.*` wrappers accept near paths, buffers, and parameter blocks in the
program segment. DOS memory allocation returns a paragraph segment, not an EZRA
pointer. Use `dos.memory.read_byte`, `write_byte`, `read_word`, `write_word`,
`copy_to_segment`, and `copy_from_segment` to access such blocks. The helpers
save and restore `ES` around far-memory operations.

A `.COM` file, its static data, and its runtime storage must remain within one
64 KiB segment. The target layout reserves the PSP and assigns bounded regions
to code, read-only data, RAM, assets, scratch storage, and a high stack window;
normal build validation rejects overflowing sections, while startup rejects a
loader block too small to separate the fixed storage from the preserved stack.

## SDK modules

The compiler embeds these modules from `toolchains/msdos-i8086/sdk/dos/`:

| Module | Purpose |
| --- | --- |
| `dos.constants` | DOS versions, PSP/FCB/DTA offsets, handles, attributes, open modes, errors, extended-error classifications, and memory policies |
| `dos.raw` | Shared carry/error state and DOS 3.0+ extended-error retrieval |
| `dos.console` | Character, direct, buffered, auxiliary, printer, and flush-and-read services |
| `dos.file` | DOS handle-based create/open/read/write/seek/metadata/IOCTL operations and complete-transfer helpers |
| `dos.directory` | Directories, drives, DTA selection, searches, free-space results, and DTA field readers |
| `dos.memory` | Paragraph allocation/resizing/freeing, allocation policy, UMB controls, and segment-aware memory access |
| `dos.datetime` | Date, time, packed field, and country-information services |
| `dos.process` | Version, vectors, break/verify state, child status, `EXEC`, termination, and TSR services |
| `dos.psp` | PSP metadata, command tail, default DTA/FCBs, JFT, environment, and FCB field helpers |

DOS 2.0 is the SDK baseline for handle-based files, directories, and memory
allocation. APIs requiring newer versions are marked at their declarations:
notably extended errors and several process/file calls require DOS 3.x,
extended open requires DOS 4.0, and UMB controls require DOS 5.0. The `EXEC`
wrapper is deliberately documented as DOS 3.0+ because DOS 2.x may destroy the
caller's `SS:SP`; safely supporting that behavior requires a dedicated
stack-independent assembly thunk.

All wrappers update `dos.raw` where DOS defines carry-based failure. Secondary
results such as seek high words, extended-open action, file date, and largest
available memory are cached only on the success or failure path where DOS
defines them. Call `raw.failed()` and then `raw.error_code()` immediately after
an operation. On DOS 3.0+, `raw.refresh_extended_error()` must likewise be
called immediately after the failed service.

## PSP command tail, DTA, and FCB overlap

The PSP command-tail length is at `PSP:0080h`; bytes begin at `0081h`. The raw
tail usually includes the command shell's leading separator. Its length excludes
the trailing carriage return, and the data is not NUL-terminated.

`PSP:0080h` is also DOS's default Disk Transfer Area. A `find_first` or
`find_next` using that DTA overwrites command-tail storage. Copy the tail with
`dos.psp.copy_command_tail` before searching, and move the DTA to a private
43-byte-or-larger buffer with `dos.directory.set_dta`. Full operations on the
default FCBs at `005Ch` and `006Ch` can also overlap the tail, especially the
second FCB. Copy arguments before using either default FCB.

The PSP environment and expanded Job File Table use far pointers. On DOS 2.x,
the JFT helpers safely use the fixed 20-byte table at PSP:18h; on DOS 3.0+ they
follow the expanded table pointer. PSP previous-parent fields require DOS 3.0+,
and the PSP version override field requires DOS 5.0+ (use `dos.process.version`
for the actual running version). The PSP helpers expose far segments and offsets
separately and use segment-aware reads;
`environment_byte` safely returns zero when the environment segment is `0000h`
or `FFFFh`.

## Examples and validation

Examples are under `examples/msdos-i8086/`:

- `hello.ezra` uses DOS console output;
- `arguments.ezra` copies and prints the raw command tail; and
- `file-io.ezra` creates, writes, closes, reopens, reads, prints, and deletes a file.

`examples/tiny-lisp` also includes `msdos-com-i8086` in its multi-target project.

Compiler tests validate target resolution, PSP/load offset `0100h`, startup and
`AH=4Ch` termination, strict original-8086 assembly, built-in SDK resolution,
CLI/API/no-std packaging, and example builds. There is currently no deterministic
DOS-emulator integration in the test suite: no installed backend was capable of
executing the generated interrupt-driven programs, and `sim86 0.1.0` lacks the
required memory, control-flow, stack, flags, and interrupt semantics. Runtime
emulator coverage remains tracked work rather than a mocked success.

## References

- Intel, [The 8086 Family User's Manual](http://matthieu.benoit.free.fr/cross/data_sheets/Intel_8086_users_manual.htm)
- Microsoft, [MS-DOS source releases](https://github.com/microsoft/ms-dos)
