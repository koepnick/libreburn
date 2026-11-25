# Editor setup (Neovim)

Autocomplete and type checking work with Neovim's built-in LSP client and no
per-project configuration (NF-9).

## Rust

`rust-analyzer` is installed by `rust-toolchain.toml`. Project settings live
in `rust-analyzer.toml` at the repo root (clippy on save, all targets).

```lua
vim.lsp.enable("rust_analyzer")  -- nvim-lspconfig
```

## Python

Run `uv sync` once; it creates `.venv` and builds the PyO3 module.
`pyrightconfig.json` points pyright (or basedpyright) at `.venv`, so imports
of `libreburn`, `libreburn._core` and PySide6 resolve.

```lua
vim.lsp.enable("pyright")
vim.lsp.enable("ruff")
```

`libreburn._core` is compiled Rust; its types come from
`python/libreburn/src/libreburn/_core.pyi`, and both packages ship `py.typed`.
`tools/check_stubs.py` fails CI if a Rust `#[pyfunction]` has no stub.
