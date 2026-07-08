# CP/M SDK Support Tracker

This ticket tracks the work needed for complete CP/M 2.2 support in EZRA. The target family is:

```text
cpm-2.2-z80
cpm-2.2-i8080
cpm-2.2-i8085
```

The current output format is `.com`, loaded at `0x0100` in the transient program area. CP/M programs call BDOS through address `0x0005` with the function number in `C` and arguments in the usual 8080/Z80 registers.

## Current Status

- `.COM` packaging exists for CP/M targets.
- CP/M source builds exist for Z80, 8080, and 8085 target profiles.
- Built-in SDK modules exist for `cpm.bdos`, `cpm.console`, `cpm.fcb`, and `cpm.dma`.
- Assembly examples exist under `examples/cpm-z80`.
- Source examples exist at `examples/cpm-z80/hello-source.ezra` and `examples/cpm-z80/file-control.ezra`.
- The VM test harness emulates BDOS 0, 2, and 9 for basic console-output tests.

## SDK Modules

Planned module set:

- `cpm.bdos`: raw BDOS constants and register-shaped wrappers.
- `cpm.console`: console-oriented helpers for character, line, status, and `$`-terminated string output.
- `cpm.fcb`: File Control Block offsets, constructors, filename helpers, extent/random-record helpers, and file result constants.
- `cpm.dma`: default DMA address constants and helpers for setting DMA buffers.
- `cpm.disk`: selected-disk, login-vector, read-only-vector, allocation-vector, reset/access/free-drive helpers.
- `cpm.user`: user-code get/set helpers.
- `cpm.serial`: reader, punch, list-device wrappers if they remain useful beyond `cpm.bdos` names.

## BDOS Coverage

`cpm.bdos` should expose every CP/M 2.2 BDOS function 0-40:

| Fn | Name | SDK status | VM status |
| ---: | --- | --- | --- |
| 0 | System reset | Wrapped | Emulated |
| 1 | Console input | Wrapped | Pending |
| 2 | Console output | Wrapped | Emulated |
| 3 | Reader input | Wrapped | Pending |
| 4 | Punch output | Wrapped | Pending |
| 5 | List output | Wrapped | Pending |
| 6 | Direct console I/O | Wrapped | Pending |
| 7 | Get I/O byte | Wrapped | Pending |
| 8 | Set I/O byte | Wrapped | Pending |
| 9 | Print `$`-terminated string | Wrapped | Emulated |
| 10 | Read console buffer | Wrapped | Pending |
| 11 | Get console status | Wrapped | Pending |
| 12 | Return version number | Wrapped | Pending |
| 13 | Reset disk system | Wrapped | Pending |
| 14 | Select disk | Wrapped | Pending |
| 15 | Open file | Wrapped | Pending |
| 16 | Close file | Wrapped | Pending |
| 17 | Search for first | Wrapped | Pending |
| 18 | Search for next | Wrapped | Pending |
| 19 | Delete file | Wrapped | Pending |
| 20 | Read sequential | Wrapped | Pending |
| 21 | Write sequential | Wrapped | Pending |
| 22 | Make file | Wrapped | Pending |
| 23 | Rename file | Wrapped | Pending |
| 24 | Return login vector | Wrapped | Pending |
| 25 | Return current disk | Wrapped | Pending |
| 26 | Set DMA address | Wrapped | Pending |
| 27 | Get allocation vector | Wrapped | Pending |
| 28 | Write-protect disk | Wrapped | Pending |
| 29 | Get read-only vector | Wrapped | Pending |
| 30 | Set file attributes | Wrapped | Pending |
| 31 | Get disk parameter block | Wrapped | Pending |
| 32 | Get/set user code | Wrapped | Pending |
| 33 | Read random | Wrapped | Pending |
| 34 | Write random | Wrapped | Pending |
| 35 | Compute file size | Wrapped | Pending |
| 36 | Set random record | Wrapped | Pending |
| 37 | Reset drive | Wrapped | Pending |
| 38 | Access drive | Wrapped | Pending |
| 39 | Free drive | Wrapped | Pending |
| 40 | Write random with zero fill | Wrapped | Pending |

## Console SDK Checklist

- Character output: `console.write`.
- Blocking character input: `console.read`.
- Non-blocking/direct console read: `console.try_read`.
- Console status: `console.key_available`.
- CR/LF newline helper: `console.newline`.
- `$`-terminated string output: `console.print_dollar`.
- `$`-terminated line output: `console.print_line_dollar`.
- Buffered line input wrapper around BDOS 10: pending.
- Decimal/hex formatting helpers: pending.
- Backspace/editing helpers for simple text UIs: pending.

## File And Disk SDK Checklist

- Define FCB offsets for drive, name, extension, extent, records, random record, and current record: `cpm.fcb` done.
- Provide result constants for success, not-found/error, and directory-full cases: partial in `cpm.fcb`.
- Provide helpers to clear and initialize 36-byte FCB buffers: `cpm.fcb` done.
- Provide helpers to set 8.3 filenames in FCBs: per-character helpers done; whole-name helpers pending.
- Wrap open, close, make, delete, rename, search-first, search-next.
- Wrap sequential read/write using the current DMA address.
- Wrap random read/write, compute-file-size, and set-random-record.
- Provide DMA buffer setup helpers and examples: `cpm.dma` and `examples/cpm-z80/file-control.ezra` done.
- Document CP/M wildcard semantics and drive numbering.

## Runtime And Tooling Checklist

- Keep `.COM` base and entry at `0x0100`.
- Keep source codegen restricted to instructions valid for the chosen CPU profile.
- Ensure Z80, 8080, and 8085 CP/M targets build source and assembly inputs.
- Expand VM BDOS emulation enough for SDK tests: console input/status, direct console I/O, buffered input, and basic file calls with in-memory fake files.
- Add source examples for console, file read, file write, and directory scan.
- Add docs for running `.COM` output in common CP/M emulators.
- Add package tests ensuring CP/M SDK files are embedded in published crates.

## Usage Notes

Normal EZRA string literals are zero-terminated. CP/M BDOS function 9 requires `$` termination. The current SDK exposes raw `u16` address wrappers for function 9; ergonomic string-literal and embed address passing is pending pointer-to-16-bit-address support.

```ezra
import cpm.console

fn main() {
    console.write('H')
    console.write('i')
    console.newline()
    console.exit()
}
```

Use `cpm.bdos` when you need direct BDOS control. Use `cpm.console` for common console apps.
