# Editor Support

Syntax definitions for EZRA live under `editors/` and cover `.ezra` and `.ezralayout` files. Editors with LSP support can run `ezrac lsp` for diagnostics and completions when `ezrac` is built or installed with `--features lsp`. The server watches EZRA sources, `Ezra.toml`, and configured layout files so import, project, and layout changes republish workspace diagnostics.

```sh
cargo install --path /path/to/ezrac/crates/ezrac-cli --features lsp
```

## Editors

- VS Code: run `npm install` in `editors/vscode`, then open it as an extension folder or package it with `vsce`. The extension starts `ezrac lsp` by default; override `ezra.languageServer.command` or `ezra.languageServer.args` if needed.
- Zed: run `ezrac install-syntax --editor zed` to install the complete extension package under Zed's data directory, then restart Zed. For development, use `zed: install dev extension` and select `editors/zed`; it includes both Ezra and Ezra Assembly grammars and starts `ezrac lsp` through the Rust extension.
- Notepad++: import `editors/notepad++/ezra.xml` through Language > User Defined Language > Import.
- Micro: copy `editors/micro/ezra.yaml` to the `syntax` directory under `$MICRO_CONFIG_HOME`, `$XDG_CONFIG_HOME/micro`, or `~/.config/micro`, in that order. For LSP, install Micro's official `lsp` plugin with `micro -plugin install lsp`, then add `"lsp.server": "ezra=ezrac lsp"` to `settings.json` or set `MICRO_LSP='ezra=ezrac lsp'`.
- Helix: run `ezrac install-syntax --editor helix`, then run `hx --grammar fetch` and `hx --grammar build` to compile both the Ezra and Ezra Assembly grammars. The installer registers `.ezra`, `.ezralayout`, and `.asm` files and installs their highlight queries. The bundled language config starts `ezrac lsp` for Ezra sources.
- Nano: include `editors/nano/ezra.nanorc` from `~/.nanorc`.
- Vim: put `editors/vim` on `runtimepath` or copy its `ftdetect`, `ftplugin`, and `syntax` directories into a Vim package. For LSP, use an LSP client such as `vim-lsp` and register `ezrac lsp` for filetype `ezra`.
- Neovim: use the Vim runtime files; see `editors/neovim/README.md` for a built-in LSP setup snippet.
- CodeMirror 6: import `ezraLanguage` from `editors/codemirror/ezra.js`.
- GitHub: GitHub does not load repository-local grammars. The root `.gitattributes` maps `.ezra` and `.ezralayout` files to Rust highlighting for now. `editors/github/languages.yml` and the TextMate grammar are starting points for an upstream Linguist contribution.
- Codeberg: Codeberg/Forgejo does not load repository-local EZRA grammars. The root `.gitattributes` provides the same Rust-highlighting fallback where the instance honors `linguist-language`; see `editors/codeberg/README.md`.

The shared keyword list is in `editors/common/ezra-keywords.txt`. The parser source of truth remains `src/ezra.pest`.
