# CI and runners

| Workflow | Job | Runner | When | Required |
| --- | --- | --- | --- | --- |
| `ci.yml` | Lint and format | GitHub-hosted ubuntu-24.04 | Every PR | Yes |
| `ci.yml` | Unit tests (x86_64) | GitHub-hosted ubuntu-24.04 | Every PR | Yes |
| `ci.yml` | Build and unit tests (aarch64) | `ARM64_RUNNER`, else hosted | Every PR | Yes |
| `ci.yml` | DCO sign-off | GitHub-hosted | Every PR | Yes |
| `container.yml` | Multi-arch image to `ghcr.io/koepnick/libreburn` | hosted amd64 + `ARM64_RUNNER` | Merges to main, tags | No |
| `hil.yml` | HIL, Ruida K40 | self-hosted `hil-ruida` | Dispatch, nightly, `hil` label | No |
| `hil.yml` | HIL, galvo | self-hosted `hil-galvo` | Dispatch, nightly, `hil` label | No |
| `release.yml` | Wheels, AppImage, Flatpak | hosted + `ARM64_RUNNER` | Version tags | No |

## aarch64

The aarch64 job cross-compiles on an x86_64 host and runs the test binaries
under QEMU user-mode (`.cargo/config.toml` sets the linker and runner). This
is much faster than a full ARM64 VM, which KVM cannot accelerate on x86.

Until the workstation runner is registered, the job runs on a GitHub-hosted
runner. To move it, register the runner with the labels `self-hosted` and
`arm64-cross`, install `gcc-aarch64-linux-gnu`, `libc6-dev-arm64-cross` and
`qemu-user`, then set the repository variable:

```
ARM64_RUNNER = ["self-hosted","arm64-cross"]
```

## Hardware tests

`hil.yml` does nothing until the repository variable `HIL_ENABLED` is `true`,
so nightly runs never queue against runners that don't exist yet. See
`hil/README.md` for runner setup and the safety rules.

## Repository settings to apply by hand

- Protect `main`: PRs only, required checks (the four `ci.yml` jobs above),
  linear history, no force pushes.
- Create the `hil` label.

## Secrets

None yet. `GITHUB_TOKEN` pushes to GHCR. A release-signing key and a Flathub
token come later.
