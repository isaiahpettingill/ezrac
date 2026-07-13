# Real libretro core integration tests

EZRA's opt-in `play96 0.3.2` integration suite rebuilds platform examples and runs the resulting content on real libretro cores. These tests are intentionally ignored during normal `cargo test` runs because the cores are third-party native libraries downloaded under their own licenses.

No core binaries or source trees are vendored in this repository. The runner stores downloads, generated disks, save states, and screenshots below the ignored `target/` directory.

## Quick start

PowerShell 7 is recommended on Windows, Linux, and macOS:

```powershell
./tools/test-real-cores.ps1
```

The script downloads the same current x86-64 core archives used by RetroArch's core updater:

- mGBA for Game Boy and Game Boy Color
- Fuse for ZX Spectrum
- ep128emu for CP/M through its Enterprise IS-DOS mode

RetroArch does not distribute ez180N. The script downloads the x86-64 core from the `nightly` release at `https://codeberg.org/josemancharo/ez180N/releases`, verifies it against the release's `SHA256SUMS`, and caches the verified binary alongside the RetroArch cores. Pass `-Ez180NCore <path>` or set `PLAY96_EZ180N_CORE` to use an existing build instead.

Run only one platform with `-Suite`:

```powershell
./tools/test-real-cores.ps1 -Suite GameBoy
./tools/test-real-cores.ps1 -Suite ZxSpectrum
./tools/test-real-cores.ps1 -Suite Cpm
./tools/test-real-cores.ps1 -Suite Ez180N
```

Use `-Refresh` to replace cached RetroArch cores with the latest buildbot versions and refresh the ez180N nightly release:

```powershell
./tools/test-real-cores.ps1 -Refresh
```

The downloader currently supports x86-64 Windows, Linux, and macOS. Other hosts can download suitable cores manually and use the direct commands below.

## Coverage

| Test | Examples | Core | Assertions |
| --- | --- | --- | --- |
| `gameboy_examples_run_on_real_core` | All five projects under `examples/gameboy` | mGBA | ROM-only header, CGB flag, Nintendo logo, and both checksums for every artifact; DMG/CGB video; sprite/background rendering; joypad-driven palette and scrolling changes; audio; and deterministic save states for every example |
| `zx_spectrum_example_runs_on_real_core` | `examples/zxspectrum-z80/hello` | Fuse | `.tap` loading, keyboard-driven `RANDOMIZE USR` fallback, visible output, the program's blue border, and save-state round trips |
| `cpm_examples_run_on_real_core` | All source and assembly programs under `examples/cpm-z80` | ep128emu | CP/M builds, IS-DOS boot and program launch, visible output, fixture file access, and deterministic save states |
| `ez180n_examples_run_on_real_core` | All three projects under `examples/ez180n` | ez180N | Character video, joypad movement/jumping, and sound output |

The CP/M suite creates a 720 KiB FAT12 disk for each generated `.com`, adds a `README.TXT` fixture for file-reading examples, and writes an adjacent ep128emu configuration selecting `EP128_DISK_ISDOS`. These generated files remain under `target/play96-cpm`.

Agon MOS and TI examples are not included yet because the repository does not currently emit content that a configured `play96`-compatible libretro core can boot directly for those systems.

Each successful test writes its final framebuffer to `target/play96-captures`. These PNGs are useful for reviewing a core's rendering when an assertion changes. The runner also writes shareable Markdown and JSON reports to `target/play96-results` with suite status, runtime, core source, and SHA-256 identity.

The latest reviewed four-suite run is published in [`docs/real-core-test-results.md`](real-core-test-results.md). Partial `-Suite` reports contain only the selected platform and should not replace that complete snapshot.

## Direct test commands

Set the relevant core environment variable and select one ignored test. PowerShell example:

```powershell
$env:PLAY96_GAMEBOY_CORE = "C:\path\to\mgba_libretro.dll"
cargo test --test libretro_examples gameboy_examples_run_on_real_core -- --ignored --exact --nocapture
```

The variables are:

- `PLAY96_GAMEBOY_CORE`
- `PLAY96_ZX_SPECTRUM_CORE`
- `PLAY96_CPM_CORE`
- `PLAY96_EZ180N_CORE`

POSIX shell example:

```sh
PLAY96_ZX_SPECTRUM_CORE=/path/to/fuse_libretro.so \
  cargo test --test libretro_examples zx_spectrum_example_runs_on_real_core -- --ignored --exact --nocapture
```

To run every manually configured platform:

```sh
cargo test --test libretro_examples -- --ignored --nocapture
```

## Failure behavior

The suite fails rather than silently skipping when a selected test's environment variable is missing or points to a nonexistent file. It deletes the expected cartridge before each build, preventing stale artifacts from making a broken compiler build look successful.

`play96` allows only one active libretro session because libretro callbacks are process-global. The test module serializes all real-core suites automatically, including when Rust's test runner uses multiple threads.

## Core compatibility notes

`play96 0.3.2` normalizes the RGB565 and 0RGB1555 frames produced by official RetroArch cores to XRGB8888 for assertions and captures. It also provides the keyboard input used to start Fuse content when Fuse fast-loads the CODE block without honoring the tape's BASIC auto-start line.

The ep128emu core requires libretro system/save/content directory callbacks during `retro_init` and frame-time callback support while running. `play96 0.3.2` supplies both, allowing the CP/M suite to run against the unmodified RetroArch buildbot binary. ep128emu may report missing external ROMs in its logs; it then falls back to its embedded default ROMs.
