# Published real-core test results

This page records the latest reviewed run of EZRA's opt-in `play96` integration suite. Core binaries are not committed; the hashes identify the exact third-party binaries used for this result.

- **Published:** 2026-07-13
- **Run generated:** `2026-07-13T22:00:40.7577832+00:00`
- **Host:** Windows x86-64
- **Frontend:** `play96 0.3.2`
- **Runner:** `tools/test-real-cores.ps1`
- **Result:** all 16 example executions passed

## Results

| Platform | Test | Examples | Core source | Result | Runtime |
| --- | --- | ---: | --- | --- | ---: |
| Game Boy / Game Boy Color | `gameboy_examples_run_on_real_core` | 5 | RetroArch buildbot mGBA | Passed | 5.94 s |
| ZX Spectrum | `zx_spectrum_example_runs_on_real_core` | 1 | RetroArch buildbot Fuse | Passed | 3.61 s |
| CP/M 2.2 Z80 | `cpm_examples_run_on_real_core` | 7 | RetroArch buildbot ep128emu | Passed | 133.49 s |
| ez180N | `ez180n_examples_run_on_real_core` | 3 | Codeberg `nightly` release | Passed | 8.87 s |

The complete four-suite publisher passed on this host and generated the timings above in one run. Fuse reported version 1.6.0 during the published run.

## Core identities

| Core binary | SHA-256 |
| --- | --- |
| `mgba_libretro.dll` | `7cefa328150bf9eb7b82da25339460b3057b12faf159f58d8ca026cf29497425` |
| `fuse_libretro.dll` | `c12fc5385649c4f07a2d85b003a15bf86310c8f95b3d784a95ec8dbe585feacd` |
| `ep128emu_core_libretro.dll` | `09c4836615d3ab31f4f525653a52799e5a518f9b9b3633d03ba658d2f32dc9c7` |
| `ez180n_windows_x64.dll` | `4a70d02437ce0ac991dbf4cf1e7d60a79893608923b89d3a64ffc18a73cd9a20` |

## Behavioral evidence

- **mGBA:** DMG/CGB video geometry and rendering, sprite/background output, joypad-driven palette and scroll changes, audio, and deterministic save states.
- **Fuse:** `.tap` loading, keyboard-driven start fallback, visible output, blue border behavior, and save-state round trip.
- **ep128emu:** all source and assembly programs built, booted from generated FAT12 IS-DOS disks, produced distinct visible output, accessed fixture files, and restored a deterministic save state.
- **ez180N:** character video, joypad movement and jumping, and sound output.

Successful runs also produced framebuffer captures under `target/play96-captures`. Those generated images and all third-party binaries remain untracked.

## Reproducing and publishing a new result

Run:

```powershell
./tools/test-real-cores.ps1 -Refresh
```

Each run writes shareable reports to:

- `target/play96-results/real-core-results.md`
- `target/play96-results/real-core-results.json`

Review those reports and captures before updating this published snapshot. A partial `-Suite` run reports only the selected suite and should not replace a complete four-suite publication.
