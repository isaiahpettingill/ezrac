# Disk Images

`ezrac disk` creates deterministic data-disk images that can hold multiple named files. The image builders are also available from `ezra::disk` under `no_std + alloc`; the library API does not read host files.

## CLI

```sh
ezrac disk [--format <format>] [--label <label>] --output <image> \
  [--file [DISK-NAME=]HOST-PATH]...
```

`--file` can be repeated. A file may also be given as a positional argument. Without `DISK-NAME=`, the host file's basename is used on the image.

Format names and platform aliases:

| Format | Aliases | Image layout | Main use |
| --- | --- | --- | --- |
| `m35fd` | `dcpu` | 448 KiB M35FD, little-endian words, FAT12 | DCPU-16 emulators |
| `m35fd-be` | `dcpu-be` | 448 KiB M35FD, big-endian words, FAT12 | DCPU-16 emulators that store words MSB first |
| `fat12-720` | `cpm` | 720 KiB FAT12, 9 sectors/track | CP/M through Enterprise IS-DOS and ep128emu; DOS |
| `fat12-1440` | `dos`, `mos` | 1.44 MiB FAT12, 18 sectors/track | MS-DOS, PC emulators, and MOS/FatFS readers |
| `d64` | `c64` | 35-track Commodore 1541 image | C64 emulators and real-drive tools |

The output extension supplies a default when `--format` is omitted:

- `.d64` selects `d64`.
- `.dsk` selects `fat12-720` for the CP/M/IS-DOS path.
- `.img` and `.IMG` select `fat12-1440`.

M35FD images also commonly use `.dsk`, so select `--format m35fd` or `--format m35fd-be` explicitly.

### Examples

Create a DCPU-16 disk containing a program and support data:

```sh
ezrac disk --format m35fd --label EZRA-DCPU --output game.dsk \
  --file BOOT.BIN=target/generic-dcpu-bare/game.bin \
  --file LEVEL1.DAT=assets/level1.bin
```

Create the 720 KiB FAT12 image used by the CP/M real-core path:

```sh
ezrac disk --format cpm --label EZRA-CPM --output game.dsk \
  --file GAME.COM=target/cpm-2.2-z80/game.com \
  --file README.TXT=README.TXT
```

This is a data disk for Enterprise IS-DOS, which runs CP/M programs and exposes FAT files. It is not a native CP/M allocation-map image and does not boot CP/M by itself.

Create a standard DOS data floppy:

```sh
ezrac disk --format dos --label EZRA-DOS --output GAME.IMG \
  --file GAME.COM=target/msdos-com-i8086/game.com \
  --file CONFIG.DAT=assets/config.dat
```

Create a Commodore 1541 disk. `.prg` files become closed PRG entries; other files become closed SEQ entries unless the library caller overrides the type:

```sh
ezrac disk --label "EZRA GAME" --output game.d64 \
  --file GAME.PRG=target/commodore64-mos6502/game.prg \
  --file LEVELS.DAT=assets/levels.bin
```

## Library API

```rust
use ezra::disk::{DiskFile, DiskFormat, DiskRequest, create_disk_image};

let files = [
    DiskFile::new("GAME.COM", game_com),
    DiskFile::new("README.TXT", b"Run GAME from drive A:\r\n"),
];
let image = create_disk_image(&DiskRequest::new(
    DiskFormat::Fat12_720K,
    "EZRA CPM",
    &files,
))?;
```

`DiskFile::with_c64_file_type` can force `C64FileType::Program` or `C64FileType::Sequential` for D64 output. The C64 type is ignored by FAT12 formats.

The API validates names, detects names that collide after uppercase disk encoding, checks directory and data capacity, and returns `DiskError` instead of truncating input.

## Format Details

### M35FD

The image follows M35FD revision `0x000b` high-density double-sided geometry:

- 2 sides
- 32 tracks
- 7 sectors per track
- 512 DCPU words, or 1024 bytes, per sector
- 448 sectors and 458,752 bytes total

The sectors contain a FAT12 superfloppy with 1024-byte logical sectors. This gives DCPU software a documented directory and file-chain format and lets little-endian images be opened by FAT-aware host tools. M35FD defines sector I/O but no standard filesystem or boot process, so a DCPU program must include or load a FAT12 reader. `BOOT.BIN` is only a normal file name; the drive does not auto-load it.

Use `m35fd` when the emulator converts each byte pair to a DCPU word as low byte then high byte. Use `m35fd-be` when it converts high byte then low byte. The big-endian image is word-swapped and is not directly mountable as FAT on the host.

Some older M35FD implementations use PC-style 80-track, 18-sector geometry instead of revision `0x000b`. DCPU-Toolchain accepts the shorter official image and zero-fills its extra sectors, but writes a 1.44 MiB file when it later saves the disk. The geometry here follows the [M35FD revision `0x000b` specification](https://github.com/techcompliant/TC-Specs/blob/a7289f204d4c9abcd6b153479e33000567eb8e29/Storage/m35fd.txt).

### FAT12

The FAT images are standard unpartitioned floppy data disks with two FAT copies, a volume-label directory entry, fixed valid timestamps, uppercase 8.3 names, and matching BPB geometry. They are deterministic and are not bootable unless callers add platform boot code and required system files separately.

The 720 KiB image matches the layout used by `tests/libretro_examples.rs` for ep128emu's `EP128_DISK_ISDOS` mode. The 1.44 MiB image is the common PC high-density format accepted by DOS emulators such as DOSBox-X, 86Box, PCem, and QEMU.

### D64

D64 output is a standard 174,848-byte, 35-track, error-byte-free 1541 image. It includes a valid BAM, a directory on track 18, and ordinary Commodore DOS sector chains. Images accept at most 144 files and 168,656 bytes of file payload. File names and labels use uppercase ASCII-compatible PETSCII and are padded with shifted spaces.

The output can be attached directly in VICE and other C64 emulators. It can also be handled by tools that support standard 35-track D64 images.
