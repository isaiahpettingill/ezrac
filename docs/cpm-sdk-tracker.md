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
- Source examples for console output, line input, command tails, record files, directory scans, and temporary-file cleanup exist under `examples/cpm-z80`.
- The compiler-host layer is record-based and has no hidden allocation or byte-stream adapter.
- The in-process CP/M 2.2 BDOS fixture covers the SDK's console, command-tail, DMA, FCB file, random-record, and directory-search paths on Z80, i8080, and i8085 tests.

## SDK Modules

Planned module set:

- `cpm.bdos`: raw BDOS constants, register-shaped wrappers, operation-specific result constants/helpers, user-area helpers, and pointer-safe record-file aliases.
- `cpm.console`: console-oriented helpers for character, line, status, `$`-terminated string output, bounded command-tail copying, and token access.
- `cpm.fcb`: File Control Block offsets, constructors, bounded 8.3 parsing/formatting, drive/user validation, wildcard helpers, rename preparation, directory-entry decoding, and record-position helpers.
- `cpm.dma`: default DMA constants, explicit 128-byte record/directory buffer setup, and buffer clearing/copy helpers.
- `cpm.disk`: selected-disk, login-vector, read-only-vector, allocation-vector, reset/access/free-drive helpers.
- `cpm.user`: user-code get/set helpers.
- `cpm.serial`: reader, punch, list-device wrappers if they remain useful beyond `cpm.bdos` names.

## BDOS Coverage

`cpm.bdos` exposes every standard CP/M 2.2 BDOS function: 0-37 and 40. Functions
38 and 39 are MP/M extensions and are also available for compatible systems.

| Fn | Name | SDK status | VM status |
| ---: | --- | --- | --- |
| 0 | System reset | Wrapped | Emulated |
| 1 | Console input | Wrapped | Emulated |
| 2 | Console output | Wrapped | Emulated |
| 3 | Reader input | Wrapped | Emulated |
| 4 | Punch output | Wrapped | Emulated |
| 5 | List output | Wrapped | Emulated |
| 6 | Direct console I/O | Wrapped | Emulated |
| 7 | Get I/O byte | Wrapped | Emulated |
| 8 | Set I/O byte | Wrapped | Emulated |
| 9 | Print `$`-terminated string | Wrapped | Emulated |
| 10 | Read console buffer | Wrapped | Emulated |
| 11 | Get console status | Wrapped | Emulated |
| 12 | Return version number | Wrapped | Emulated |
| 13 | Reset disk system | Wrapped | Emulated |
| 14 | Select disk | Wrapped | Emulated |
| 15 | Open file | Wrapped | Emulated |
| 16 | Close file | Wrapped | Emulated |
| 17 | Search for first | Wrapped | Emulated |
| 18 | Search for next | Wrapped | Emulated |
| 19 | Delete file | Wrapped | Emulated |
| 20 | Read sequential | Wrapped | Emulated |
| 21 | Write sequential | Wrapped | Emulated |
| 22 | Make file | Wrapped | Emulated |
| 23 | Rename file | Wrapped | Emulated |
| 24 | Return login vector | Wrapped | Pending |
| 25 | Return current disk | Wrapped | Emulated |
| 26 | Set DMA address | Wrapped | Emulated |
| 27 | Get allocation vector | Wrapped | Pending |
| 28 | Write-protect disk | Wrapped | Pending |
| 29 | Get read-only vector | Wrapped | Pending |
| 30 | Set file attributes | Wrapped | Emulated |
| 31 | Get disk parameter block | Wrapped | Pending |
| 32 | Get/set user code | Wrapped | Emulated |
| 33 | Read random | Wrapped | Emulated |
| 34 | Write random | Wrapped | Emulated |
| 35 | Compute file size | Wrapped | Emulated |
| 36 | Set random record | Wrapped | Emulated |
| 37 | Reset drive | Wrapped | Emulated |
| 38 | Access drive | Wrapped | Pending |
| 39 | Free drive | Wrapped | Pending |
| 40 | Write random with zero fill | Wrapped | Emulated |

## Console SDK Checklist

- Character output: `console.write`.
- Blocking character input: `console.read`.
- Non-blocking/direct console read: `console.try_read`.
- Console status: `console.key_available`.
- CR/LF newline helper: `console.newline`.
- `$`-terminated string output: `console.print_dollar`.
- `$`-terminated line output: `console.print_line_dollar`.
- Buffered line input wrapper around BDOS 10: `console.read_line`.
- Command-tail length, bounded copy, and byte access: `console.command_tail_length`, `console.copy_command_tail`, and `console.command_tail_byte`.
- Bounded command-tail token count, length, byte, and copy access: `console.command_tail_token_*`.
- Decimal/hex formatting helpers: pending.
- Backspace/editing helpers for simple text UIs: pending.

## File And Disk SDK Checklist

- Define FCB offsets for drive, name, extension, extent, records, random record, and current record: `cpm.fcb` done.
- Provide raw-result and operation-specific success, not-found, EOF, disk-full, and directory-full constants/helpers: `cpm.bdos` done.
- Provide helpers to clear and initialize 36-byte FCB buffers: `cpm.fcb` done.
- Provide bounded whole-name 8.3 parsing, validation, formatting, drive prefixes, and wildcard setup: `cpm.fcb` done. The parser accepts a conservative ASCII CP/M character subset and folds lowercase to uppercase.
- Wrap open, close, create, delete, rename, search-first, and search-next with pointer forms: `cpm.bdos` done.
- Wrap sequential read/write using the current DMA address: `cpm.bdos` done.
- Wrap random read/write, compute-file-size, and set-random-record: `cpm.bdos` done.
- Expose only 128-byte record seek and byte-offset conversion. A byte-stream seek adapter, partial-record buffering, and hidden allocation are intentionally unsupported.
- Decode directory search results into names, drive, user area, and completion status: `cpm.fcb` done. Search results point into one 128-byte DMA record with four 32-byte entries.
- Provide explicit record and directory DMA setup helpers: `cpm.dma` done.
- Document CP/M wildcard semantics, drive numbering, user areas, temporary files, DMA lifetime, and compiler-host memory limits: this tracker documents them below.

## Runtime And Tooling Checklist

- Keep `.COM` base and entry at `0x0100`.
- Keep source codegen restricted to instructions valid for the chosen CPU profile.
- Ensure Z80, 8080, and 8085 CP/M targets build source and assembly inputs.
- Keep the in-process BDOS fixture and its Z80/i8080/i8085 tests aligned with the record-based SDK. The fixture borrows caller-owned fixed record arrays; it does not allocate program file storage.
- Add source examples for console, arguments, record copy/seek, directory scan, and temporary-file cleanup.
- Add docs for running `.COM` output in common CP/M emulators.
- Add package tests ensuring CP/M SDK files are embedded in published crates.

## Compiler-host limits and conventions

Normal EZRA string literals are zero-terminated. CP/M BDOS function 9 requires `$` termination. The SDK exposes raw `u16` address wrappers for function 9 and pointer-based wrappers for buffered input, FCB, and DMA operations.

- The low 256 bytes are owned by CP/M. BDOS is entered through `0x0005`; the `.COM` program loads and starts at `0x0100`.
- The initial command tail is at `0x0080`: byte `0` is its length and bytes `1..length` are the tail. It is not NUL-terminated. `0x0080` is also the initial/default DMA address, so copy the tail before any operation that changes DMA or uses it for a record/search result.
- Every sequential or random file operation transfers one 128-byte record through the current DMA address. Directory search also writes one 128-byte record, containing four 32-byte directory entries. DMA contents are owned by the next BDOS operation and must be copied if they need to survive it.
- Keep FCBs in program-owned memory. A normal FCB is 36 bytes. BDOS rename uses bytes `0..11` for the old name and `16..27` for the new name; `fcb.prepare_rename` fills the latter range.
- There is no byte-stream file API in this layer. Use `fcb.set_record_position` followed by `bdos.apply_record_position_at` with 128-byte record numbers. `fcb.record_for_byte_offset` and `fcb.byte_offset_in_record` are arithmetic helpers only; partial-record buffering remains the caller's responsibility.
- CP/M drive numbers are `0` for the current/default drive and `1..16` for `A:` through `P:`. A default-drive FCB stays at zero and resolves through BDOS function 25; `fcb.directory_entry_drive_for_current` converts that selector for display. User areas are global BDOS state, `0..15`, and are not stored in an FCB. Use `bdos.get_user` and `bdos.set_user`.
- In a search FCB, `?` matches one name or extension position. The usual `*` shorthand means the remaining positions are wildcards; build that form with `fcb.wildcard_name`, `fcb.wildcard_extension`, or `fcb.set_wildcard`. The bounded parser accepts `?` but rejects `*` so a literal parse cannot silently broaden a search. Directory name and extension attribute high bits are masked when entries are decoded.
- A safe temporary-file convention is an application-owned 8.3 name with a `.$$$` extension, such as `STAGE001.$$$`. Treat `create` returning `0xFF` as a collision or create failure; never overwrite a collision. Close and delete the temporary file on failure. After a complete close, optionally prepare the rename FCB and call BDOS rename to commit it to the final name. Cleanup after a successful rename is not a delete of the final file.
- The current EZRA CP/M layout reserves `0x0100..0x7FFF` for code, `0x8000..0x9FFF` for read-only data, `0xA000..0xBFFF` for RAM, `0xC000..0xDFFF` for assets, `0xE000..0xEFFF` for scratch, and `0xF000..0xFFFF` for the stack. These are packaging assumptions, not a portable CP/M memory query. A system with a smaller TPA cannot run an image that reaches beyond its resident BDOS/BIOS area; leave room for the stack and all live FCB/DMA/scratch buffers.
- Stock CP/M 2.2 has no general child-process, `exec`, or wait API. `bdos.child_execution_supported()` and `bdos.command_chaining_supported()` explicitly return false. CCP `SUBMIT` or system-specific chaining commands are non-portable and are not wrapped here; a compiler host must return to the CCP and let it perform any command sequencing.

The command-tail, directory, and temporary-file examples under `examples/cpm-z80` show the intended order of operations.

```ezra
import cpm.console

fn main() {
    console.write('H')
    console.write('i')
    console.newline()
    console.exit()
}
```

Use `cpm.bdos` when you need direct BDOS control. Use `cpm.console` for common console apps. VM tests can call `run_assembly_test_with_cpm_bdos_fixture` with fixed caller-owned record arrays, a command-tail slice, and a console-input slice.
