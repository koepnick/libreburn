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
| `release.yml` | Wheels, sdists, AppImage, Flatpak, provenance; GitHub release, PyPI, crates.io | hosted + `ARM64_RUNNER` | Version tags | No |

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

See the owner setup checklist in [supply-chain.md](supply-chain.md): merge
settings, the `main` and `v*` rulesets (including signed commits), the
`release`, `pypi` and `crates-io` environments, and the `hil` label.

## Secrets and variables

`GITHUB_TOKEN` pushes to GHCR, and OIDC covers Sigstore signing, attestations,
PyPI and crates.io, so most of CI needs no stored secrets.

| Name | Kind | Scope | Purpose |
| --- | --- | --- | --- |
| `RELEASE_GPG_PRIVATE_KEY` | secret | `release` environment | Detached GPG signatures on release files; skipped while unset |
| `RELEASE_GPG_PASSPHRASE` | secret | `release` environment | Passphrase for that key, if any |
| `PUBLISH_PYPI` | variable | repository | `true` once PyPI trusted publishers exist |
| `PUBLISH_CRATES` | variable | repository | `true` once crates.io trusted publishing is set up |
| `ARM64_RUNNER` | variable | repository | See aarch64 above |
| `HIL_ENABLED` | variable | repository | See hardware tests above |

A Flathub token comes later.

## Workflow rules

- Pin every action to a full commit SHA with the version in a comment
  (`uses: owner/action@<sha> # v1.2.3`). Dependabot updates both.
- Give each job only the `permissions:` it needs; the workflow default is
  `contents: read`.
- Check out with `persist-credentials: false`.
- Never hand a secret to a job that compiles code.

The "Lint and format" job runs zizmor over the workflows, and actionlint
reads `.github/actionlint.yaml` for the self-hosted runner labels.
