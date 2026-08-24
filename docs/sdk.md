# Built-in SDKs

Target SDKs are EZRA source modules embedded into the compiler binary from `toolchains/*/sdk`. Import them like ordinary modules:

```ezra
import agon.console

fn main() {
    console.print_line("Hello from EZRA")
}
```

The exact modules depend on the target. Examples include:

- `agon.mos`, `agon.console`, `agon.vdp`, `agon.buffers`, `agon.sprites`, `agon.keyboard`, `agon.mouse`, and `agon.gpio` for Agon MOS.
- `cpm.*` for CP/M.
- `dos.*` for the MS-DOS `.COM` target.
- `gb.*` for Game Boy.
- `ez180n.console` for ez180N.
- `zx.*` for ZX Spectrum.
- `tice.*` for TI CE profiles.
- `arduboy.*` for Arduboy.
- `dcpu.*` for DCPU-16.

Run `ezrac targets` to see the SDK family for each documented target. The source files under `toolchains/` are the most precise reference for exported names and signatures.

## Project SDK modules

Project SDK paths are searched before bundled modules:

```toml
[sdk]
paths = ["sdk", "../shared-sdk"]
```

For `import device.video`, the compiler looks for `device/video.ezra` under each configured root. Public declarations need `pub` to be visible to importers. Keep target-specific implementations behind [`@cfg`](language/conditional-compilation.md) when one module must support more than one target.

## Intrinsic catalogs

`ezra.bits`, `ezra.int`, and `ezra.mem` are compiler-provided intrinsic catalogs, not editable SDK source files. They are imported normally:

```ezra
import ezra.int
import ezra.mem

fn quotient(value: u16) -> u16 {
    let result: u16, remainder: u16 = ezra.int.divmod(value, 3u16)
    return result
}
```

The catalogs provide width-aware bit operations, integer helpers, overlap-aware memory operations, explicit-endian loads/stores, and scalar byte access. Exact widths and ABI support remain target-dependent. The [language diagnostics](language/diagnostics.md) page lists the two-result and memory rules.

## SDK limitations

SDK calls are target APIs, not portable standard-library calls. A module may exist while its runtime behavior is only Tier 3 or Tier 4 support. Check [platform support](platforms.md) and the target guide before depending on an API. Agon applications should return from `main` to MOS rather than use emulator-only exit ports.
