#!/usr/bin/env python3
"""Build a four-disk 2.88 MiB FAT12 FreeDOS test set."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path

SECTOR_SIZE = 512
TOTAL_SECTORS = 5760
SECTORS_PER_CLUSTER = 2
RESERVED_SECTORS = 1
FAT_COUNT = 2
SECTORS_PER_FAT = 9
ROOT_ENTRIES = 240
ROOT_SECTORS = (ROOT_ENTRIES * 32 + SECTOR_SIZE - 1) // SECTOR_SIZE
DATA_START_SECTOR = RESERVED_SECTORS + FAT_COUNT * SECTORS_PER_FAT + ROOT_SECTORS
IMAGE_SIZE = SECTOR_SIZE * TOTAL_SECTORS
CLUSTER_SIZE = SECTOR_SIZE * SECTORS_PER_CLUSTER
MEDIA_DESCRIPTOR = 0xF0

INSTALL_BAT = b"""@echo off\r\nif not exist C:\\EZRAC md C:\\EZRAC\r\ncopy A:\\SETUP.BAT C:\\EZRAC\\SETUP.BAT >NUL\r\nC:\r\ncd \\EZRAC\r\nSETUP.BAT\r\n"""

SETUP_BAT = b"""@echo off\r\necho Installing EZRAC for FreeDOS in C:\\EZRAC\r\ncopy A:\\EZFE.EXE C:\\EZRAC\\EZFE.EXE >NUL\r\ncopy A:\\EZRAC.BAT C:\\EZRAC\\EZRAC.BAT >NUL\r\ncopy A:\\HELLO.EZR C:\\EZRAC\\HELLO.EZR >NUL\r\n:disk2\r\necho Insert EZRAC disk 2 of 4, then press a key.\r\npause >NUL\r\nif not exist A:\\EZOPT.EXE goto disk2\r\ncopy A:\\EZOPT.EXE C:\\EZRAC\\EZOPT.EXE >NUL\r\n:disk3\r\necho Insert EZRAC disk 3 of 4, then press a key.\r\npause >NUL\r\nif not exist A:\\EZCG.EXE goto disk3\r\ncopy A:\\EZCG.EXE C:\\EZRAC\\EZCG.EXE >NUL\r\n:disk4\r\necho Insert EZRAC disk 4 of 4, then press a key.\r\npause >NUL\r\nif not exist A:\\EZAS.EXE goto disk4\r\ncopy A:\\EZAS.EXE C:\\EZRAC\\EZAS.EXE >NUL\r\necho.\r\necho Compiling HELLO.EZR...\r\nC:\r\ncd \\EZRAC\r\ncall EZRAC.BAT HELLO.EZR HELLO.COM\r\nif errorlevel 1 goto failed\r\necho.\r\necho Running HELLO.COM...\r\nHELLO.COM\r\ngoto done\r\n:failed\r\necho HELLO.EZR did not compile.\r\n:done\r\n"""


def dos_name(name: str) -> bytes:
    path = Path(name)
    stem = path.stem.upper()
    suffix = path.suffix[1:].upper()
    if not stem or len(stem) > 8 or len(suffix) > 3:
        raise ValueError(f"not an 8.3 DOS filename: {name}")
    return stem.ljust(8).encode("ascii") + suffix.ljust(3).encode("ascii")


def set_fat12_entry(fat: bytearray, cluster: int, value: int) -> None:
    offset = cluster + cluster // 2
    if cluster & 1:
        fat[offset] = (fat[offset] & 0x0F) | ((value << 4) & 0xF0)
        fat[offset + 1] = (value >> 4) & 0xFF
    else:
        fat[offset] = value & 0xFF
        fat[offset + 1] = (fat[offset + 1] & 0xF0) | ((value >> 8) & 0x0F)


def build_image(label: str, files: list[tuple[str, bytes]]) -> bytes:
    image = bytearray(IMAGE_SIZE)
    boot = memoryview(image)[:SECTOR_SIZE]
    boot[0:3] = b"\xEB\x3C\x90"
    boot[3:11] = b"EZRACDOS"
    struct.pack_into("<H", boot, 11, SECTOR_SIZE)
    boot[13] = SECTORS_PER_CLUSTER
    struct.pack_into("<H", boot, 14, RESERVED_SECTORS)
    boot[16] = FAT_COUNT
    struct.pack_into("<H", boot, 17, ROOT_ENTRIES)
    struct.pack_into("<H", boot, 19, TOTAL_SECTORS)
    boot[21] = MEDIA_DESCRIPTOR
    struct.pack_into("<H", boot, 22, SECTORS_PER_FAT)
    struct.pack_into("<H", boot, 24, 36)
    struct.pack_into("<H", boot, 26, 2)
    struct.pack_into("<I", boot, 28, 0)
    struct.pack_into("<I", boot, 32, 0)
    boot[36] = 0
    boot[38] = 0x29
    struct.pack_into("<I", boot, 39, 0x455A5241)
    boot[43:54] = label[:11].ljust(11).encode("ascii")
    boot[54:62] = b"FAT12   "
    boot[510:512] = b"\x55\xAA"

    fat = bytearray(SECTORS_PER_FAT * SECTOR_SIZE)
    fat[0:3] = bytes((MEDIA_DESCRIPTOR, 0xFF, 0xFF))
    root = bytearray(ROOT_SECTORS * SECTOR_SIZE)
    root[0:11] = label[:11].ljust(11).encode("ascii")
    root[11] = 0x08

    next_cluster = 2
    for entry_index, (name, contents) in enumerate(files, start=1):
        cluster_count = max(1, (len(contents) + CLUSTER_SIZE - 1) // CLUSTER_SIZE)
        first_cluster = next_cluster
        last_cluster = first_cluster + cluster_count - 1
        data_end_cluster = 2 + (TOTAL_SECTORS - DATA_START_SECTOR) // SECTORS_PER_CLUSTER
        if last_cluster >= data_end_cluster:
            raise ValueError(f"{name} does not fit on the floppy")
        for cluster in range(first_cluster, last_cluster + 1):
            set_fat12_entry(fat, cluster, 0xFFF if cluster == last_cluster else cluster + 1)
        data_offset = (DATA_START_SECTOR * SECTOR_SIZE) + (first_cluster - 2) * CLUSTER_SIZE
        image[data_offset : data_offset + len(contents)] = contents

        offset = entry_index * 32
        root[offset : offset + 11] = dos_name(name)
        root[offset + 11] = 0x20
        struct.pack_into("<H", root, offset + 26, first_cluster)
        struct.pack_into("<I", root, offset + 28, len(contents))
        next_cluster = last_cluster + 1

    fat_start = RESERVED_SECTORS * SECTOR_SIZE
    for index in range(FAT_COUNT):
        start = fat_start + index * len(fat)
        image[start : start + len(fat)] = fat
    root_start = (RESERVED_SECTORS + FAT_COUNT * SECTORS_PER_FAT) * SECTOR_SIZE
    image[root_start : root_start + len(root)] = root
    return bytes(image)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release-dir", type=Path, default=Path("target/dos/release"))
    parser.add_argument("--hello", type=Path, default=Path("../examples/msdos-i8086/hello/hello.ezra"))
    parser.add_argument("--output-dir", type=Path, default=Path("target/freedos-floppies"))
    args = parser.parse_args()

    release = args.release_dir
    required = ["ezfe.exe", "ezopt.exe", "ezcg.exe", "ezas.exe", "EZRAC.BAT"]
    missing = [name for name in required if not (release / name).is_file()]
    if missing:
        raise SystemExit(f"missing DOS build outputs: {', '.join(missing)}")
    if not args.hello.is_file():
        raise SystemExit(f"missing hello-world source: {args.hello}")

    disks = [
        (
            "EZRAC1",
            [
                ("INSTALL.BAT", INSTALL_BAT),
                ("SETUP.BAT", SETUP_BAT),
                ("EZRAC.BAT", (release / "EZRAC.BAT").read_bytes()),
                ("HELLO.EZR", args.hello.read_bytes()),
                ("EZFE.EXE", (release / "ezfe.exe").read_bytes()),
            ],
        ),
        ("EZRAC2", [("EZOPT.EXE", (release / "ezopt.exe").read_bytes())]),
        ("EZRAC3", [("EZCG.EXE", (release / "ezcg.exe").read_bytes())]),
        ("EZRAC4", [("EZAS.EXE", (release / "ezas.exe").read_bytes())]),
    ]

    args.output_dir.mkdir(parents=True, exist_ok=True)
    for number, (label, files) in enumerate(disks, start=1):
        output = args.output_dir / f"ezrac-{number}.img"
        output.write_bytes(build_image(label, files))
        print(f"wrote {output} ({IMAGE_SIZE} bytes, {label})")


if __name__ == "__main__":
    main()
