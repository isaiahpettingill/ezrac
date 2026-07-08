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
ezrac test [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>
```

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

`--cpu <mode>` selects assembly syntax and opcode validation for assembly input. Supported modes are `i8080`, `i8085`, `z80`, `z80n`, `z180`, and `ez80`.

`--base <addr>` assembles at an explicit base address. Addresses may be decimal, `0x` hexadecimal, or `h`-suffixed hexadecimal.

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
[build].output          output format: bin, com, hex, 8xp, 8ek, or 8xk
[build].input_kind      ezra or assembly
[build].assembler_cpu   i8080, i8085, z80, z80n, z180, or ez80
[build].executable      artifact basename and TI variable/app name source
[layout].file           custom .ezralayout file
[sdk].paths             additional SDK source roots
```

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
hex, ihex, intel-hex Intel HEX text
8xp, ti8xp          TI protected program file
8ek, ti8ek          TI CE app-style file
8xk, ti8xk          classic TI app-style file
```

Target defaults:

```text
CP/M targets                 com
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
cargo run -- assemble --target cpm-2.2-z80 examples/cpm-z80/hello-char.asm
```

Build a CP/M source example:

```sh
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/hello-source.ezra
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
ezrac assemble --target cpm-2.2-z80 --map hello.map examples/cpm-z80/hello-char.asm
ezrac build --target cpm-2.2-z80 --input-kind assembly examples/cpm-z80/hello-char.asm
```

The assembler accepts the implemented 8080, 8085, Z80, Z80N, Z180, and eZ80 subset. See `docs/ez80-opcode-coverage.md` for opcode coverage notes.

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
