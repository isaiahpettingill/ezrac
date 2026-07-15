# Published real-core test results

This page records the latest visually reviewed run of EZRA's opt-in `play96` integration suite. Core binaries are not committed; the hashes identify the exact third-party binaries used for this result.

- **Published:** 2026-07-15
- **Run generated:** `2026-07-15T23:43:34.9717684+00:00`
- **Host:** Windows x86-64
- **Frontend:** `play96 0.3.2`
- **Runner:** `tools/test-real-cores.ps1 -Suite All`
- **Result:** all 20 example executions passed across five suites

## Results

| Platform | Test | Examples | Core source | Result | Runtime |
| --- | --- | ---: | --- | --- | ---: |
| Arduboy | `arduboy_snake_runs_on_real_core` | 1 | RetroArch buildbot Arduous | Passed | 5.25 s |
| Game Boy / Game Boy Color | `gameboy_examples_run_on_real_core` | 6 | RetroArch buildbot mGBA | Passed | 9.59 s |
| ZX Spectrum | `zx_spectrum_examples_run_on_real_core` | 2 | RetroArch buildbot Fuse | Passed | 11.04 s |
| CP/M 2.2 Z80 | `cpm_examples_run_on_real_core` | 7 | RetroArch buildbot ep128emu | Passed | 172.39 s |
| ez180N | `ez180n_examples_run_on_real_core` | 4 | Codeberg `nightly` release | Passed | 16.45 s |

## Core identities

| Core binary | SHA-256 |
| --- | --- |
| `arduous_libretro.dll` | `b8ab90d589e47d0daef10fd37fa1be51ddc84f87d0de5aa6e9dd3a9b7e10ffcf` |
| `mgba_libretro.dll` | `7fa6c6e0a5ffa86affeb4c21987896c95e63945a66facb64c86eee4b1771c38f` |
| `fuse_libretro.dll` | `30db2a703d18760a6ff20a8ca9e0037f0dad19c2ccdd4465c17f5a42b0a10ea0` |
| `ep128emu_core_libretro.dll` | `b1e434a8e7da3e945910e04510ae3bc79195c60da12f048d168c79ce537fdf16` |
| `ez180n_windows_x64.dll` | `4a70d02437ce0ac991dbf4cf1e7d60a79893608923b89d3a64ffc18a73cd9a20` |

## Visual review

- **Arduous:** Snake board, snake, food, score, and controls render clearly at 128×64.
- **mGBA:** checkerboard backgrounds, sprite, CGB palette grid, Mandelbrot silhouette, audio/input screen, and serial boot diagnostic were reviewed.
- **Fuse:** hello output and blue border render correctly; Mandelbrot's generated orbit pattern fills the complete 256×192 bitmap.
- **ep128emu:** source and assembly programs launch from generated IS-DOS disks; `Hi`, `Hello from EZRA on CP/M`, `Type:`, and the fixture byte `E` are visible as expected.
- **ez180N:** hello, jumping, Mandelbrot, and Meteor Run screens render correctly; input and audio assertions pass.

Agon MOS and TI-99/4A examples were separately build-validated because no available Play96-compatible core can directly boot their emitted artifacts. C64 hello and Mandelbrot were tested with current Windows cores, but are not published as successful runtime results: Frodo returned corrupt framebuffers and VICE x64/x64sc crashed during session startup.

Generated screenshots remain under the ignored `target/play96-captures` directory. Reproduce the complete report with:

```powershell
./tools/test-real-cores.ps1 -Suite All -Refresh
```
