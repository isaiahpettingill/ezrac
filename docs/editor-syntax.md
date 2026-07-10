# Editor Support

Syntax definitions for EZRA live under `editors/` and cover `.ezra` and `.ezralayout` files. Editors with LSP support can run `ezrac lsp` for diagnostics and completions when `ezrac` is built or installed with `--features lsp`. The server watches EZRA sources, `Ezra.toml`, and configured layout files so import, project, and layout changes republish workspace diagnostics.

```sh
cargo install --path /path/to/ezrac --features lsp
```

## Editors

- VS Code: run `npm install` in `editors/vscode`, then open it as an extension folder or package it with `vsce`. The extension starts `ezrac lsp` by default; override `ezra.languageServer.command` or `ezra.languageServer.args` if needed.
- Zed: install `editors/zed` as a development extension. It uses the local `editors/tree-sitter-ezra` grammar and starts `ezrac lsp` through the Rust extension.
- Notepad++: import `editors/notepad++/ezra.xml` through Language > User Defined Language > Import.
- Micro: copy `editors/micro/ezra.yaml` to Micro's syntax directory. For LSP, install Micro's official `lsp` plugin with `micro -plugin install lsp`, then add `"lsp.server": "ezra=ezrac lsp"` to `settings.json` or set `MICRO_LSP='ezra=ezrac lsp'`.
- Helix: merge `editors/helix/languages.toml` into your Helix config and copy `editors/helix/queries/highlights.scm` to the Ezra query directory after building the grammar. The bundled language config starts `ezrac lsp`.
- Nano: include `editors/nano/ezra.nanorc` from `~/.nanorc`.
- Vim: put `editors/vim` on `runtimepath` or copy its `ftdetect`, `ftplugin`, and `syntax` directories into a Vim package. For LSP, use an LSP client such as `vim-lsp` and register `ezrac lsp` for filetype `ezra`.
- Neovim: use the Vim runtime files; see `editors/neovim/README.md` for a built-in LSP setup snippet.
- CodeMirror 6: import `ezraLanguage` from `editors/codemirror/ezra.js`.
- GitHub: GitHub does not load repository-local grammars. The root `.gitattributes` maps `.ezra` and `.ezralayout` files to Rust highlighting for now. `editors/github/languages.yml` and the TextMate grammar are starting points for an upstream Linguist contribution.
- Codeberg: Codeberg/Forgejo does not load repository-local EZRA grammars. The root `.gitattributes` provides the same Rust-highlighting fallback where the instance honors `linguist-language`; see `editors/codeberg/README.md`.

The shared keyword list is in `editors/common/ezra-keywords.txt`. The parser source of truth remains `src/ezra.pest`.
