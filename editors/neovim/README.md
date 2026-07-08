# Neovim

Neovim can use the Vim runtime files in `../vim`:

```vim
set runtimepath+=/path/to/ezrac/editors/vim
```

For package-style installation, copy or symlink `../vim/ftdetect`, `../vim/ftplugin`, and `../vim/syntax` into a directory on Neovim's `runtimepath`.

## LSP

Install `ezrac` with LSP support enabled and make sure `ezrac lsp` is on `PATH`:

```sh
cargo install --path /path/to/ezrac --features lsp
```

With Neovim's built-in LSP client, add this to `init.lua`:

```lua
vim.api.nvim_create_autocmd({ "BufRead", "BufNewFile" }, {
  pattern = { "*.ezra", "*.ezralayout" },
  callback = function()
    vim.bo.filetype = "ezra"
  end,
})

vim.api.nvim_create_autocmd("FileType", {
  pattern = "ezra",
  callback = function()
    vim.lsp.start({
      name = "ezra-lsp",
      cmd = { "ezrac", "lsp" },
      root_dir = vim.fs.root(0, { "Ezra.toml", ".git" }) or vim.fn.getcwd(),
    })
  end,
})
```
