# Codeberg

Codeberg/Forgejo does not load repository-local EZRA syntax grammars. The root `.gitattributes` maps `.ezra` and `.ezralayout` files to Rust for repository-host highlighting until EZRA has upstream highlighter support.

```gitattributes
*.ezra linguist-language=Rust
*.ezralayout linguist-language=Rust
```

If a Codeberg instance does not honor `linguist-language`, syntax highlighting must wait for upstream Chroma/Forgejo support.
