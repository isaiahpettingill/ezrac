# EZRA Compiler Usage

`ezrac` is the compiler and tooling binary for EZRA. When developing from this repository, replace `ezrac` with `cargo run --` in the examples below.

## Installation And Local Development

Build and test the compiler with Cargo:

```sh
cargo build
cargo test --quiet
```

Run commands from the repository without installing:

```sh
cargo run -- check examples/agon-mos/hello/src/main.ezra
cargo run -- build examples/agon-mos/hello/src/main.ezra
```

After installing or copying the binary into your `PATH`, run the same commands with `ezrac`:

```sh
ezrac check path/to/main.ezra
ezrac build path/to/main.ezra
```

## Commands

Print command help:

```sh
ezrac --help
```

Create a new project scaffold:

```sh
ezrac init [--name <name>] [--target <triple>] [--force] [dir]
```

Install syntax highlighting for selected editors:

```sh
ezrac install-syntax (--all | [--editor] <editor>...) [--dry-run]
```

List documented target triples, default outputs, and SDKs:

```sh
ezrac targets
```

Validate a source file:

```sh
ezrac check [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>
```

Emit target assembly to stdout:

```sh
ezrac emit-asm [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>
```

Emit inspectable IR text to stdout:

```sh
ezrac emit-ir [--stage hir|tbir] [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>
```

Build source or assembly artifacts:

```sh
ezrac build [--target <triple>] [--cpu <mode>] [--input-kind ezra|assembly] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] [file.ezra|file.asm]
```

Run generated code in the compiler's target VM test path:

```sh
ezrac test [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] [file.ezra]
```

With no file argument, `test` loads `Ezra.toml` from the current directory, discovers `tests/**/*.ezra` in deterministic path order, builds each artifact, and reports a per-test result plus a CI-friendly summary. Target selection is `--target`, then `[test].target`, then `[build].target`, then the compiler default.

The built-in test runner uses the `ez80` emulator backend for eZ80 ADL, Z80,
Z80N, Z180, i8080, and i8085 target profiles. eZ80 uses a 24-bit ADL address
space; the other built-in CPU modes use 16-bit address and stack bounds. The
runner backend interface is extensible, so new CPU families can supply an
emulator without changing the compiler test command.

Assemble handwritten assembly:

```sh
ezrac assemble [--target <triple>] [--cpu <mode>] [--layout <file.ezralayout>] [--map <file.map>] [--base <addr>] [--output <file.bin>] <file.asm>
```

Print a layout summary:

```sh
ezrac layout [file.ezralayout]
```

Print the default 64-byte EZRA cartridge header:

```sh
ezrac header
```

## Common Options

`--target <triple>` selects the platform profile. If omitted, the default is `custom-unknown-ez80`.

Use `ezrac targets` to list the target triples with documented layouts and SDKs. Pattern entries such as `cpm-*-z80` accept concrete versions like `cpm-2.2-z80`.

`--layout <file.ezralayout>` replaces the target's default memory layout.

`--debug-comments` includes extra comments in generated assembly.

`--no-default-sdk-symbols` prevents automatic default SDK/runtime symbols when the target would normally provide them.

`--input-kind ezra|assembly` overrides input detection for `build`. Without it, `.ezra` is treated as source and `.asm`, `.s`, `.z80`, `.ez80`, `.i8080`, and `.8080` are treated as assembly.

`--cpu <mode>` selects assembly syntax and opcode validation for assembly input. Default builds support `i8080`, `i8085`, `z80`, `z80n`, `z180`, `ez80`, and `lr35902`. Enable optional processor families with Cargo features: `avr`, `m6800`, `m68k`, `mos6502`, or `tms9900` (for example, `cargo run --features tms9900 -- assemble --target bare-tms9900 program.asm`). AVR is assembly-only; TMS9900 provides handwritten assembly plus the initial scalar source backend and `ti99-4a-tms9900` cartridge target. TMS9900 syntax and scope are documented in [`tms9900-assembly.md`](tms9900-assembly.md).

`--base <addr>` assembles at an explicit base address. Addresses may be decimal, `0x` hexadecimal, or `h`-suffixed hexadecimal.

## Assembly Macros

Handwritten assembly is preprocessed before CPU-specific parsing for both
`ezrac assemble`, including `--base`, and `ezrac build --input-kind assembly`.
This layer is target-independent: macro sets can be vendored with an assembly
project and used by eZ80, Z80-family, and Intel-family assembly inputs.

Use ordinary relative `include` directives to vendor a macro set:

```asm
include "macros/console.inc"
%print_char 65
```

Defines substitute `${NAME}` in ordinary assembly and macro bodies:

```asm
%define DEBUG_PORT 0Ch
out (${DEBUG_PORT}), a
```

Conditionals select source based on the configured target or assembler CPU:

```asm
%if cpu("ez80")
    out0 (0Ch), a
%else
    out (0Ch), a
%endif
```

Macros have named parameters referenced as `$name`. Invoke a macro with a `%`
prefix so it cannot conflict with an instruction mnemonic. Labels beginning
with `%%` are hygienic per invocation.

```asm
%macro delay(count)
%%loop:
    djnz %%loop
%endmacro

%delay 16
```

Supported condition forms are `cpu("name")`, `target("triple")`,
`feature("name")`, and `defined(NAME)`. `feature` tests the Cargo feature set
that compiled `ezrac` (for example, `feature("m68k")`). Macros expand recursively up to 32 levels. The macro layer
does not compile or link `.ezra` SDK functions; reusable assembly APIs should
be published as vendorable macro sets with explicit target ABI requirements.

## ZX Spectrum Output

`zxspectrum-z80` builds produce a `.tap` containing a standard CODE header and data block. The code loads and starts at `0x8000`, so common Spectrum emulators can load the tape directly. If the emulator does not auto-start CODE blocks, use `LOAD "" CODE` followed by `RANDOMIZE USR 32768`.

## Initializing Projects

Use `init` to create a starter project:

```sh
ezrac init my-game
ezrac init --name coffee-order --target agonlight-mos-ez80 examples/coffee-order
```

The command creates:

```text
.gitignore
Ezra.toml
README.md
src/main.ezra
sdk/.gitkeep
assets/.gitkeep
```

It refuses to overwrite existing files by default. Pass `--force` to replace scaffold files.

The generated `src/main.ezra` is target-aware for built-in SDK targets. For example, `agonlight-mos-ez80` imports `agon.console`, CP/M targets import `cpm.console`, and generic targets create a minimal empty `main`.

## Installing Syntax Highlighting

Use `install-syntax` to copy bundled syntax files into local editor configuration directories. Select editors explicitly or use `--all`:

```sh
ezrac install-syntax vim neovim
ezrac install-syntax --editor vscode --editor zed
ezrac install-syntax --all
ezrac install-syntax --all --dry-run
```

Supported editor names:

```text
vim
neovim, nvim
nano
micro
helix, hx
vscode, code
zed
notepad++, notepadpp, npp
```

Install locations are best-effort defaults:

```text
Vim          ~/.vim/{ftdetect,ftplugin,syntax}/ezra.vim
Neovim       ~/.config/nvim/{ftdetect,ftplugin,syntax}/ezra.vim
Nano         ~/.nano/ezra.nanorc and an include line in ~/.nanorc
Micro        ~/.config/micro/syntax/ezra.yaml
Helix        ~/.config/helix/languages.toml and runtime/queries/ezra/highlights.scm
VS Code      ~/.vscode/extensions/ezra-language
Zed          ~/.config/zed/extensions/ezra
Notepad++    %APPDATA%/Notepad++/userDefineLangs/ezra.xml, or ~/.config/Notepad++ on non-Windows
```

Some editors may require restart, extension reload, or additional grammar build steps after files are copied. See `docs/editor-syntax.md` for manual installation notes.

## Project Files

EZRA projects use `Ezra.toml`. `ezrac` searches for the nearest `Ezra.toml` from the source file's directory upward.

```toml
[project]
name = "my-program"

[build]
input = "src/main.ezra"
target = "agonlight-mos-ez80"
output = "bin"
input_kind = "ezra"
assembler_cpu = "ez80"
executable = "my-program"

[layout]
file = "layouts/custom.ezralayout"

[sdk]
paths = ["sdk", "../shared-sdk"]
```

Supported fields:

```text
[build].input           default source path when running `ezrac build` without a file
[build].target          target triple
[build].output          output format: bin, com, gaem, hex, tap, gb, prg, crt, 8xp, 8ek, or 8xk
[build].input_kind      ezra or assembly
[build].assembler_cpu   i8080, i8085, z80, z80n, z180, ez80, lr35902, avr, m6800, m68k, 6502, or tms9900 (optional families require their Cargo feature)
[build].executable      artifact basename and TI variable/app name source
[layout].file           custom .ezralayout file
[sdk].paths             additional SDK source roots
[lsp].mode              application (default) or library
```

`[lsp].mode = "library"` makes the language server type-check the configured source and its SDK imports without requiring `fn main()`. It does not add shared-library output; `build` remains executable-only.

The parser also accepts a `[cartridge]` table with `layout` and optional `manifest`, but cartridge packaging is still evolving.

## Build Artifacts

`ezrac build` writes three artifacts:

```text
<name>.asm        generated or copied assembly
<name>.map        section and symbol map
<name>.<ext>      executable image for the selected output format
```

If the source belongs to a project, artifacts are written under:

```text
<project>/target/<target>/<source-relative-directory>/
```

Without a project file, artifacts are written under a `target` directory next to the input file.

The artifact basename defaults to the source file stem. `[build].executable` overrides it.

## Output Formats

Supported output format names:

```text
bin                 raw binary bytes
com                 CP/M .COM image
gaem                ez180N cartridge image
hex, ihex, intel-hex Intel HEX text
tap, zxtap          ZX Spectrum tape image
gb, gameboy         Game Boy ROM image
prg, c64            Commodore 64 program image
crt, commodore64-crt Commodore 64 standard cartridge image
8xp, ti8xp          TI protected program file
8ek, ti8ek          TI CE app-style file
8xk, ti8xk          classic TI app-style file
```

Target defaults:

```text
ez180N Libretro Console       gaem
CP/M targets                 com
ZX Spectrum targets          tap
Game Boy targets             gb (`.gbc` filename for CGB builds)
Commodore 64 target          prg
Arduboy AVR targets          hex
TI calculator targets        8xp
all other targets            bin
```

Agon MOS targets use `bin` as the format name but wrap the code in the Agon MOS executable structure.

## Building Examples

Build the Agon hello example:

```sh
cargo run -- build examples/agon-mos/hello/src/main.ezra
```

Build the Agon SDK showcase:

```sh
cargo run -- build examples/agon-mos/sdk-showcase/src/main.ezra
```

Assemble a CP/M example:

```sh
cargo run -- build --target cpm-2.2-z80 --input-kind assembly examples/cpm-z80/console-output.asm
```

Build a CP/M source example:

```sh
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/console-output.ezra
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/line-input.ezra
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/file-read.ezra
```

## SDK Imports

Built-in SDK modules are target-specific. They are embedded into the compiler binary from `toolchains/*/sdk`.

Use imports like ordinary EZRA modules:

```ezra
import agon.console

fn main() {
    console.println("Hello from EZRA")
}
```

Project SDK paths are searched before built-in SDK modules:

```toml
[sdk]
paths = ["sdk"]
```

Given `import device.video`, `ezrac` looks for `device/video.ezra` under each SDK root.

## Assembly Input

Use `assemble` for direct one-file assembly output, or `build --input-kind assembly` to route assembly through the normal target artifact layout.

```sh
ezrac assemble --target cpm-2.2-z80 --map console-output.map examples/cpm-z80/console-output.asm
ezrac build --target cpm-2.2-z80 --input-kind assembly examples/cpm-z80/console-output.asm
```

The assembler accepts implemented instruction subsets for 8080, 8085, Z80, Z80N, Z180, eZ80, LR35902, and MOS 6502. Optional assemblers are available for AVR, M6800, M68k, and TMS9900 when built with their Cargo features. AVR is assembly-only; TMS9900 also has the initial scalar source backend and TI-99/4A cartridge profile. See [`tms9900-assembly.md`](tms9900-assembly.md) for TMS9900 syntax and scope. See `docs/ez80-opcode-coverage.md` for Zilog-family opcode coverage notes.

## Custom Layouts

Use `ezrac layout` to inspect a layout:

```sh
ezrac layout
ezrac layout layouts/custom.ezralayout
```

Use the layout with any build-like command:

```sh
ezrac build --layout layouts/custom.ezralayout src/main.ezra
```

Layouts define load, entry, stack, memory regions, output sections, and named symbols used by generated code.

## Editor Support

Syntax-highlighting assets are documented in `docs/editor-syntax.md`. The shared parser source of truth is `src/ezra.pest`.


## Motorola 6800 assembler

Standalone Motorola 6800 assembly is available with `ezrac assemble --cpu m6800 --target bare-m6800`; see [m6800-assembly.md](m6800-assembly.md) for syntax and instruction coverage.
