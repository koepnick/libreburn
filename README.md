# libreburn

Linux-first, ARM64-ready laser control for hobbyists and hackers. Version 1
targets an OmTech K40 with a Ruida controller and a fiber galvo on a JCZ
EZCAD2 (LMC) board, on x86_64 and on a Raspberry Pi 5.

> Early scaffold. The drivers talk to mock devices only; nothing here drives a
> real laser yet.

## Layout

```
crates/libreburn-core/          project model, geometry, planner, estimates, safety
crates/libreburn-driver-ruida/  Ruida UDP and USB driver + mock
crates/libreburn-driver-lmc/    EZCAD2 LMC USB driver + mock
crates/libreburn-service/       service binary (libreburnd), HTTP + WebSocket API
crates/libreburn-py/            PyO3 bindings, Python module libreburn._core
python/libreburn/               PyPI package libreburn: SDK, imports as `libreburn`
python/libreburn-desktop/       PySide6 client + IPython console (command: libreburn)
profiles/                       machine profiles (TOML)
materials/                      example materials library
hil/                            hardware-in-the-loop tests and fixtures
packaging/                      Containerfile, Flatpak manifest, AppImage recipe
docs/
```

The service (`libreburnd`) is the only process that talks to a controller.
Clients, including the desktop app and the Python SDK, use its API.

## Build and test

Needs Rust (the version in `rust-toolchain.toml` installs itself through
rustup) and [uv](https://docs.astral.sh/uv/).

```sh
cargo nextest run --workspace     # or: cargo test --workspace
uv sync                           # builds the PyO3 module and installs both Python packages
uv run pytest
uv run libreburn                  # desktop client
cargo run -p libreburn-service    # service on 127.0.0.1:7878
```

Cross-compile and test for aarch64 on an x86_64 host (needs
`gcc-aarch64-linux-gnu` and `qemu-user`):

```sh
cargo test --workspace --exclude libreburn-py --target aarch64-unknown-linux-gnu
```

## Laser safety

- Framing never fires the main laser source unless the user opts in for the
  session. Galvo framing uses the red guide light. The check runs in the
  service and again in each driver.
- Hardware-in-the-loop profiles cap laser power at zero.

## Docs

- [CI and runners](docs/ci.md)
- [Editor setup (Neovim)](docs/editor-setup.md)
- [Protocol sources and credits](docs/protocols.md)
- [Contributing](CONTRIBUTING.md)

## License

[AGPL-3.0-or-later](LICENSE). Contributions are accepted under the same
license with a [DCO](https://developercertificate.org/) sign-off.
