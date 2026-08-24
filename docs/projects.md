# Projects and `Ezra.toml`

A project is a directory containing `Ezra.toml`. When a source path is supplied, `ezrac` searches from that file's directory upward for the nearest project file. A command without a file uses the project in the current directory.

## Minimal project

```toml
[project]
name = "my-program"

[build]
target = "agonlight-mos-ez80"
output = "bin"
executable = "my-program"
```

Create the same shape with:

```sh
ezrac init --name my-program my-program
```

## Configuration sections

```toml
[build]
input = "src/main.ezra"
target = "agonlight-mos-ez80"
output = "bin"
input_kind = "ezra"
assembler_cpu = "ez80"
executable = "my-program"

[test]
target = "ezra-test-flat-ez80"

[optimization]
level = 2
enable = ["tail-calls"]
disable = ["function-inlining"]

[layout]
file = "layouts/custom.ezralayout"

[sdk]
paths = ["sdk", "../shared-sdk"]

[assets]
section = ".assets"
align = 16

[lsp]
mode = "application"
```

`[build].input` supplies the source for `ezrac build` without a positional path. `output` selects a package format; the target supplies a default when it is omitted. `input_kind = "assembly"` selects the handwritten assembly path. `assembler_cpu` selects the assembly syntax and validator.

`[test].target` is used by project test discovery. CLI `--target` wins over project settings, then `[test].target` wins over `[build].target` for tests. `[layout].file` replaces the target layout for build-like commands. `[sdk].paths` adds project SDK roots before bundled target SDKs.

`[lsp].mode = "library"` lets LSP checks omit `fn main()`. It does not produce a library artifact; `build` remains executable-only.

## Optimization settings

The level is `0` through `3`, defaulting to `2`. The CLI and TOML use the same pass names:

```text
scalar-simplification
local-propagation
loop-invariant-code-motion
known-bits
memory-read-licm
function-inlining
dead-code-elimination
tail-calls
tail-recursion
idempotent-operations
redundant-register-copies
```

A pass in `disable` wins over `level` and `enable`. Dead-code elimination remains enabled at level 0 unless disabled explicitly.

## SDK and assets

Given `import device.video`, an SDK root containing `device/video.ezra` can provide the module. Built-in target SDKs are embedded from `toolchains/*/sdk` and are searched after project SDK roots.

Indexed PNG conversion is configured separately from the source declaration:

```toml
[[assets.images]]
path = "assets/player.png"
kind = "sprite"
```

```ezra
embed player: bytes = file("assets/player.png")
```

See [embedded data and image assets](language/embeds-assets.md) and [image assets](image-assets.md).

## Output directories

For a project, build artifacts are written under:

```text
<project>/target/<target>/<source-relative-directory>/
```

Without a project file, they go in a `target` directory next to the input. The artifact basename is the source stem unless `[build].executable` is set. See [CLI](cli.md#build) for the `.asm`, `.map`, `.size`, and executable files.
