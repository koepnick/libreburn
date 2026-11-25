# Supply-chain security

What protects libreburn's source and release files, how users check them, and
the one-time setup only the repository owner can do.

## What is in place

| Layer | Mechanism | Where |
| --- | --- | --- |
| Commits | SSH-signed commits, verified by GitHub | Contributor setup below; branch rule on `main` |
| GitHub Actions | Every action pinned to a full commit SHA, `persist-credentials: false` on checkouts, least-privilege `permissions:` per job | `.github/workflows/` |
| Workflow audit | [zizmor](https://docs.zizmor.sh/) runs in the required "Lint and format" job | `ci.yml` |
| Dependencies | Dependabot for cargo, uv, actions and base images, with a 7-day cooldown on new releases | `.github/dependabot.yml` |
| Container base images | Pinned by digest | `packaging/container/Containerfile` |
| Build tools | appimagetool and the AppImage runtime pinned to a release and checked by SHA-256 | `packaging/appimage/build.sh` |
| Release files | GitHub artifact attestations (SLSA build provenance) for every AppImage, Flatpak, wheel and sdist | `release.yml`, job `attest` |
| Release files | `SHA256SUMS`, plus GPG detached signatures once the release key exists | `release.yml`, job `publish` |
| Container image | Keyless Sigstore (cosign) signature and a provenance attestation on the multi-arch digest | `container.yml` |
| PyPI | Trusted Publishing (OIDC, no API token) with PEP 740 attestations | `release.yml`, job `pypi` |
| crates.io | Trusted Publishing (OIDC, no API token) | `release.yml`, job `crates` |

Jobs that compile code never receive a secret. The GPG key is only exposed to
the `publish` job, which downloads finished files and builds nothing, so a
compromised build script or dependency cannot reach it. For the same reason
AppImages are not signed with an embedded signature (that would need the key
inside the build job); they get a detached `.asc` instead.

## Verifying a download

```sh
# Any release file: proves it was built by this repo's release workflow.
gh attestation verify libreburn-x86_64.AppImage --repo koepnick/libreburn

# Container image: Sigstore signature, then provenance.
cosign verify ghcr.io/koepnick/libreburn:latest \
  --certificate-identity-regexp '^https://github\.com/koepnick/libreburn/\.github/workflows/container\.yml@' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
gh attestation verify oci://ghcr.io/koepnick/libreburn:latest --repo koepnick/libreburn

# GPG (once releases carry .asc files). Check the fingerprint against the one below.
gpg --import libreburn-release-key.asc
gpg --verify SHA256SUMS.asc SHA256SUMS && sha256sum --check --ignore-missing SHA256SUMS
```

Release signing key fingerprint: *not created yet.*

## Signing your commits (SSH)

SSH signing is simpler than GPG and GitHub verifies it the same way. Use a
dedicated key; a hardware key (`ed25519-sk`) is best if you have one.

```sh
ssh-keygen -t ed25519 -C "git signing" -f ~/.ssh/git_signing   # or -t ed25519-sk
git config --global gpg.format ssh
git config --global user.signingkey ~/.ssh/git_signing.pub
git config --global commit.gpgsign true
git config --global tag.gpgsign true
```

Then add `~/.ssh/git_signing.pub` on GitHub under **Settings → SSH and GPG keys
→ New SSH key**, with key type **Signing Key** (a key added as an
authentication key does not verify signatures). The address in `git config
user.email` must be a verified email on your GitHub account.

To check signatures locally, list trusted keys:

```sh
echo "$(git config user.email) $(cat ~/.ssh/git_signing.pub)" >> ~/.config/git/allowed_signers
git config --global gpg.ssh.allowedSignersFile ~/.config/git/allowed_signers
git log --show-signature -1
```

Sign release tags too: `git tag -s v0.1.0 -m "libreburn 0.1.0"`.

## Requiring signed commits on `main`

Turn this on, but merge pull requests with **Squash and merge** only.

GitHub signs the commits it creates for squash merges and merge commits with
its own key, and commits made by Dependabot or through the web UI and API are
signed the same way. It does not sign the commits it rewrites during
**Rebase and merge**: they get the merger as committer and lose their
signature. The first two commits on `main` (`621273f`, `c8fd473`) are
unsigned for exactly that reason, although their branch commits were signed.
With "Require signed commits" on, GitHub blocks rebase merges, so set the
repository to squash only. Squash also keeps the linear history rule
satisfied, and the default squash message keeps every `Signed-off-by:` line.

Claude's commits in this project are SSH-signed and show as Verified on
GitHub, so this rule does not block its pull requests.

## Owner setup checklist

These are settings and secrets only the repository owner can change. Nothing
breaks before they are done: the publish jobs are skipped and the GPG step is
inert.

1. **Merge settings** (Settings → General → Pull Requests): keep *Allow squash
   merging*, turn off *Allow rebase merging* and *Allow merge commits*. Turn on
   *Allow auto-merge* if PRs should merge themselves when checks pass.
2. **Branch ruleset for `main`** (Settings → Rules → Rulesets → New branch
   ruleset, target `main`): require a pull request, require status checks
   (`Lint and format`, `Unit tests (x86_64)`, `Build and unit tests (aarch64)`,
   `DCO sign-off`), require linear history, **require signed commits**, block
   force pushes and deletions.
3. **Tag ruleset for `v*`** (new tag ruleset, target `refs/tags/v*`): restrict
   creation, update and deletion to yourself, so only you can trigger a release.
4. **Environments** (Settings → Environments): create `release`, `pypi` and
   `crates-io`. For each, set *Deployment branches and tags* to *Selected* with
   the tag pattern `v*`. Optionally add yourself as a required reviewer so each
   release waits for your click.
5. **Release GPG key** (optional, for detached signatures):
   ```sh
   gpg --quick-gen-key "libreburn releases <you@example.org>" ed25519 sign 2y
   gpg --armor --export-secret-keys <FPR>   # paste into the secret below
   ```
   Keep the primary copy offline or on a hardware token. In the `release`
   environment add the secret `RELEASE_GPG_PRIVATE_KEY` (and
   `RELEASE_GPG_PASSPHRASE` if the key has one). Publish the fingerprint in
   this file and the README, and upload the public key to
   keys.openpgp.org.
6. **PyPI Trusted Publishing**: on pypi.org, *Your account → Publishing → Add a
   pending publisher*, once for `libreburn` and once for `libreburn-desktop`:
   owner `koepnick`, repository `libreburn`, workflow `release.yml`,
   environment `pypi`. Then set the repository variable `PUBLISH_PYPI=true`.
   Pending publishers reserve the name and need no first manual upload.
7. **crates.io Trusted Publishing**: crates.io only allows this for crates that
   already exist. Publish the first version once from your machine
   (`cargo login`, then `cargo publish --workspace`), then on each crate's
   *Settings → Trusted Publishing* add repository `koepnick/libreburn`,
   workflow `release.yml`, environment `crates-io`. Revoke the API token,
   optionally enable *require trusted publishing* per crate, and set the
   repository variable `PUBLISH_CRATES=true`.
8. **GHCR package**: after the first image push, check the package's
   visibility at github.com/koepnick?tab=packages (public, so users can pull
   and verify).

## Maintenance notes

- `dtolnay/rust-toolchain` is pinned to a commit on its `stable` branch and
  `flathub-infra/flatpak-github-actions` to a commit on `master`; Dependabot
  only follows tagged releases, so refresh these two by hand now and then.
  The Flatpak builder container image digest in `release.yml` is also manual.
- `uvx zizmor@<version>` in `ci.yml` is pinned by version; bump it by hand.
- To bump appimagetool or the AppImage runtime, change the versions and the
  SHA-256 values together in `packaging/appimage/build.sh`.
