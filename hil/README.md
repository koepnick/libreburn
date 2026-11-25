# Hardware-in-the-loop tests

These run against real controllers on self-hosted runners labelled
`hil-ruida` and `hil-galvo`, by manual dispatch, nightly, or on a PR with the
`hil` label (see `.github/workflows/hil.yml`). They stay off until the
repository variable `HIL_ENABLED` is `true`.

Safety rules:

- Tests check protocol replies, parameter reads and motion only.
- Every profile in `hil/profiles/` sets `max_power_pct = 0` as a hard limit,
  on top of the removed keys on the test machines. `hil/run.sh` checks this
  before any hardware test runs, and the profile loader rejects a HIL profile
  with any other value.
- Galvo framing uses the guide light only.
- One job at a time per machine (a `concurrency` group per runner).
- Once the GPIO wiring is in, `hil/run.sh` also refuses to run unless the
  machine's interlock reads closed, and fails on any laser-activity signal
  (`hil/guard/`). It is off until enabled; see
  [docs/lab-setup.md](../docs/lab-setup.md#hil-guard-day-2).

Runner setup: the HIL runner lives on the Pi 5 and is provisioned by
`infra/lab/provision.sh` as an unprivileged user with laser access through
groups and udev rules only. See [docs/lab-setup.md](../docs/lab-setup.md),
which also covers isolating the Ruida's network segment and setting
`LIBREBURN_RUIDA_HOST`.

`fixtures/` holds recorded protocol exchanges from the real boards, which the
mock transports replay in normal CI (NF-7).
