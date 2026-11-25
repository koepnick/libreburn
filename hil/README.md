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

Runner setup:

- Run the runner as an unprivileged user, with udev rules for the LMC USB
  device and the Ruida USB serial port (`packaging/udev/`).
- Put the Ruida board on a separate network segment that only the test
  workstation can reach. Set its address in the runner's environment as
  `LIBREBURN_RUIDA_HOST`; it is not a secret.

`fixtures/` holds recorded protocol exchanges from the real boards, which the
mock transports replay in normal CI (NF-7).
