# Editor Syntax Highlighting

Syntax definitions for EZRA live under `editors/` and cover `.ezra` and `.ezralayout` files.

## Editors

- VS Code: open `editors/vscode` as an extension folder or package it with `vsce`.
- Zed: install `editors/zed` as a development extension. It uses the local `editors/tree-sitter-ezra` grammar.
- Notepad++: import `editors/notepad++/ezra.xml` through Language > User Defined Language > Import.
- Micro: copy `editors/micro/ezra.yaml` to Micro's syntax directory.
- Helix: merge `editors/helix/languages.toml` into your Helix config and copy `editors/helix/queries/highlights.scm` to the Ezra query directory after building the grammar.
- Nano: include `editors/nano/ezra.nanorc` from `~/.nanorc`.
- Vim: put `editors/vim` on `runtimepath` or copy its `ftdetect`, `ftplugin`, and `syntax` directories into a Vim package.
- Neovim: use the Vim runtime files; see `editors/neovim/README.md`.
- CodeMirror 6: import `ezraLanguage` from `editors/codemirror/ezra.js`.
- GitHub: GitHub does not load repository-local grammars. The root `.gitattributes` maps `.ezra` and `.ezralayout` files to Rust highlighting for now. `editors/github/languages.yml` and the TextMate grammar are starting points for an upstream Linguist contribution.
- Codeberg: Codeberg/Forgejo does not load repository-local EZRA grammars. The root `.gitattributes` provides the same Rust-highlighting fallback where the instance honors `linguist-language`; see `editors/codeberg/README.md`.

The shared keyword list is in `editors/common/ezra-keywords.txt`. The parser source of truth remains `src/ezra.pest`.
