# Contributing

## Sign-off (DCO)

libreburn uses the [Developer Certificate of Origin](https://developercertificate.org/)
instead of a CLA. Sign off every commit:

```sh
git commit -s
```

CI checks that each commit has a `Signed-off-by:` line matching its author.

## Signed commits

`main` requires signed commits. Sign with an SSH key (`git config gpg.format
ssh`, `commit.gpgsign true`) and add it to GitHub as a *Signing Key*; setup
steps are in [docs/supply-chain.md](docs/supply-chain.md#signing-your-commits-ssh).
The sign-off line above and the cryptographic signature are separate: you need
both.

## License headers

Every source file starts with:

```
SPDX-License-Identifier: AGPL-3.0-or-later
```

`python3 tools/check_spdx.py` checks this, and CI runs it.

## Before opening a PR

```sh
pre-commit install          # once
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
uv run ruff check . && uv run ruff format --check . && uv run pyright && uv run pytest
```

Add the `hil` label to a PR that needs hardware tests.

## Language and dependency rules

Rust for the service and drivers, Python (PySide6) for the desktop client and
SDK. Do not add wxPython, Electron or other web runtimes, JavaScript or
TypeScript, Go, Ruby, C# or PHP.

## Safety

Changes must not let framing or preview fire the main laser source without the
user's per-session opt-in, and must not raise the power cap in `hil/profiles/`.
