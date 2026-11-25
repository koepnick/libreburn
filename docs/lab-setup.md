# Lab machines: provisioning, runners and the HIL guard

How to set up the self-hosted machines that run libreburn's ARM64 builds and
hardware-in-the-loop (HIL) tests, with the least privilege that works.

## The short version

- **OS:** headless Debian 13 on the x86_64 workstation; Raspberry Pi OS Lite
  64-bit (Debian 13 based) on the Pi 5.
- **Runners** each run as their own locked-down system user, in a sandboxed
  systemd unit, with no sudo and no `docker` group (that group is root).
- **The agent user** (`claude`, which runs the Claude CLI) gets *scoped*
  passwordless sudo: package installs through a checked wrapper, and
  start/stop/restart of the runner. Not full sudo. Why, and how to widen it,
  is [below](#the-agent-user-and-sudo).
- **Laser access** is by group (`dialout`, `plugdev`) and udev rules only.
- **HIL runs on the Pi**, which sits next to both lasers and has the GPIO for
  the interlock guard. The workstation does the ARM64 cross-builds.
- `infra/lab/provision.sh` does all of the above and is safe to re-run.

## Machines and roles

| Machine | OS | Runner user | Labels | Workflows |
| --- | --- | --- | --- | --- |
| Workstation (x86_64) | Debian 13, headless | `ghr-arm64` | `self-hosted`, `arm64-cross` | `ci.yml` aarch64 job, arm64 halves of `container.yml` and `release.yml` (via `ARM64_RUNNER`) |
| Raspberry Pi 5, 4 GB | Raspberry Pi OS Lite 64-bit | `ghr-hil` | `self-hosted`, `hil-ruida`, `hil-galvo`, `pi5` | `hil.yml` |

One HIL runner serves both lasers, so the Pi only ever builds one job at a
time (two parallel Rust builds would not fit in 4 GB). Its unit is capped at
80% of RAM so a runaway build is killed instead of the Pi.

## Security model

| Account | Can | Cannot |
| --- | --- | --- |
| `ghr-arm64` | Build, run QEMU, use its own **rootless** Docker | sudo; read `/home`; see other users' processes; write outside its home; open devices |
| `ghr-hil` | Open the LMC (plugdev) and USB serial (dialout); reach the Ruida segment | sudo; read `/home`; touch GPIO; write outside its home |
| `claude` | `sudo lb-apt install <pkg>`, `sudo lb-apt update/upgrade`, `sudo systemctl start/stop/restart` the runner; read the journal | Other sudo; open laser devices or GPIO; reach the Ruida segment; read runner tokens; use Docker (unless `--agent-docker`, which is rootless) |
| `libreburn-hil-guard` service | Read GPIO, stop/start the HIL runner | Network, file writes outside `/run/libreburn-hil`, any capability |

Why persistent runners rather than ephemeral ones: true ephemeral runners need
a long-lived GitHub admin token on the box to mint a registration for each
job, and that token is a bigger prize than the runner itself. Instead each
runner is a persistent, sandboxed, unprivileged service (the runner kills a
job's leftover processes when it ends), and fork PRs never reach these
machines (`ci.yml` and `hil.yml` route them to hosted runners or skip them).

## Steps only you can do

1. Install the OSes: Debian 13 netinst on the workstation with only "SSH
   server" and "standard system utilities"; Raspberry Pi OS Lite 64-bit on the
   Pi (set your user and SSH key in Raspberry Pi Imager). Put your SSH public
   key in `~/.ssh/authorized_keys` on both before provisioning, because the
   script turns off SSH passwords once it sees one.
2. Workstation, if you want ARM64 container builds there: install Docker from
   [Docker's apt repository](https://docs.docker.com/engine/install/debian/)
   including `docker-ce-rootless-extras`. You don't need to add anyone to the
   `docker` group; the runner gets its own rootless daemon.
3. Clone the repository on each machine and run the provisioning (next
   section).
4. Register each runner with a token from GitHub (below).
5. In **Settings → Actions → General**, set *Fork pull request workflows from
   outside collaborators* to **Require approval for all external
   contributors**. The repository is public, so this is the outer gate.
6. Set the repository variables `ARM64_RUNNER` and, once the Pi is up,
   `HIL_ENABLED` (below).
7. Network: give the lab machines their own VLAN or firewall zone, and put
   the Ruida on its own segment ([Network isolation](#network-isolation)).
8. Confirm how the Ruida is connected. Ethernet needs nothing more. USB
   serial: add the adapter's IDs from `lsusb` to
   `packaging/udev/70-libreburn.rules` and set `transport = "serial"` in
   `hil/profiles/hil-omtech-k40-ruida.toml`.
9. As the `claude` user, install and sign in to the Claude CLI.
10. Day 2: wire and commission the GPIO guard, then turn it on
    ([HIL guard](#hil-guard-day-2)).

## Provisioning

Run from a checkout of this repository, on each machine:

```sh
# Workstation
sudo infra/lab/provision.sh host --role workstation --firewall

# Pi 5 (Ruida on a direct cable to the Pi's Ethernet; see Network isolation)
sudo infra/lab/provision.sh host --role pi --firewall \
  --ruida-net 10.77.0.0/30 --ruida-host 10.77.0.2
```

What it does: installs the build packages for the role, enables unattended
security upgrades, creates the runner user and its sandboxed unit
(`libreburn-runner@<user>.service`), sets up rootless Docker for the
workstation runner, installs the udev rules and the (disabled) HIL guard on
the Pi, creates the `claude` user if missing with scoped sudo
(`/etc/sudoers.d/50-libreburn-agent`), turns off SSH passwords and root login
if your user has an SSH key, and with `--firewall` installs a host firewall
that drops inbound traffic except SSH from `--ssh-allow` (default: the
private IPv4 ranges).

Other options: `--agent-user NAME`, `--agent-hardware` (let the agent open the
laser devices), `--agent-docker` (rootless Docker for the agent),
`--ssh-allow 192.168.10.0/24`. Run `infra/lab/provision.sh` with no arguments
for the list.

## Register the runner

1. Open the [actions/runner releases](https://github.com/actions/runner/releases),
   pick the latest version, and copy the SHA-256 for
   `actions-runner-linux-x64-<version>.tar.gz` (workstation) or
   `actions-runner-linux-arm64-<version>.tar.gz` (Pi) from the release notes.
2. Run, and paste the registration token from **Settings → Actions → Runners
   → New self-hosted runner** when asked (it is not echoed, and expires in an
   hour):

   ```sh
   sudo infra/lab/provision.sh runner --role workstation --version 2.xyz.0 --sha256 <hash>
   sudo infra/lab/provision.sh runner --role pi --version 2.xyz.0 --sha256 <hash>
   ```

   The runner updates itself after this, and the tarball's checksum is checked
   before anything runs.
3. Set the repository variables (**Settings → Secrets and variables → Actions
   → Variables**):

   | Variable | Value | Effect |
   | --- | --- | --- |
   | `ARM64_RUNNER` | `["self-hosted","arm64-cross"]` | aarch64 CI, container and release jobs move to the workstation |
   | `HIL_ENABLED` | `true` | `hil.yml` starts running (dispatch, nightly, `hil` label) |

Logs: `journalctl -u libreburn-runner@ghr-hil -f`. To re-register, delete
`/var/lib/libreburn-runners/<user>/runner/.runner` and run the `runner` step
again.

## Network isolation

- Put the workstation and the Pi on a lab VLAN (or firewall zone) that can
  reach the internet for GitHub, crates.io, PyPI and Debian mirrors, and that
  your other networks cannot reach except SSH from your admin machine.
- Keep the Ruida off every shared network. Simplest: a direct cable from the
  Pi's Ethernet to the Ruida, with static addresses (for example Pi
  `10.77.0.1/30`, Ruida `10.77.0.2`), and the Pi on Wi-Fi or a USB Ethernet
  adapter for the lab VLAN. With `--ruida-net`, the Pi's firewall lets only
  root and `ghr-hil` reach that segment (not `claude`), and routes nothing to
  or from it.
- The LMC galvo is USB only, so it needs no network rule.
- No inbound access from the internet to either machine. The runners only
  make outbound HTTPS connections to GitHub.

## The agent user and sudo

The concern is not you; it's that the agent reads untrusted text (web pages,
issues, PR comments, dependency READMEs), and a prompt injection in any of it
can make the agent run commands. On a box wired to lasers, full passwordless
sudo turns any such injection into root, which can rewrite the HIL guard, the
udev rules and the power-cap check. The scoped rule lets the agent do the
thing you actually wanted (install packages) without that.

What the agent can run:

```sh
sudo lb-apt update
sudo lb-apt upgrade
sudo lb-apt install <package>...     # archive package names only
sudo systemctl restart libreburn-runner@ghr-hil.service   # also start, stop
```

`lb-apt` exists because `apt-get install *` in sudoers is root for anyone:
`apt-get install -o APT::Update::Pre-Invoke::=sh foo` or a local `.deb` runs
arbitrary code as root. `lb-apt` accepts only plain package names from your
configured, signed repositories, and logs each call to the journal
(`journalctl -t lb-apt`). Sudo also records the session (`log_output`).

### Widening the agent's sudo

Put additions in your own file so re-provisioning never overwrites them, and
always check with `visudo -cf` before installing:

```sh
sudoedit /etc/sudoers.d/60-local
# e.g.  claude ALL=(root) NOPASSWD: /usr/bin/systemctl restart libreburnd.service
```

From most to least contained:

1. **Specific commands** with fixed arguments, as above. Avoid `*` in
   arguments: sudoers wildcards also match spaces and extra options, so
   `systemctl restart *` lets the agent pass any unit and any flag.
2. **Full sudo with a password:** `claude ALL=(ALL) ALL` and set a password
   for `claude` that only you know. The agent stops and asks you, and you
   type it in. Good on the workstation for occasional admin work.
3. **Full passwordless sudo:** `claude ALL=(ALL) NOPASSWD: ALL`. Only on the
   workstation, if ever. On the Pi it hands the lasers' safety software to
   whatever the agent last read.

## HIL guard (day 2)

Off by default. With `enabled = false` in `/etc/libreburn/hil-guard.toml`,
HIL runs on the zero power cap and the removed keys alone, and each run
prints that it is unguarded. Once wired and turned on:

- `hil/run.sh` refuses to start unless the machine's harness is plugged in,
  its interlock reads closed and its laser-activity input is off, and stops
  the tests if the interlock opens.
- Any laser activity during a run fails the job (HIL profiles cap power at
  zero, so activity means a bug).
- Independently of job code, the guard service stops the HIL runner whenever
  an interlock opens or a harness is unplugged, and starts it again after 5 s
  of safe readings. Any laser activity stops the runner and *latches* it off
  until you restart the guard. If the guard itself stops, the runner stops
  with it.

### Pins

All inputs are active low with the Pi's internal pull-ups, so a broken wire
reads as "interlock open" or "harness missing", which is the safe failure.

| Machine | Signal | GPIO | Header pin | Reads safe when |
| --- | --- | --- | --- | --- |
| Ruida K40 | Interlock | GPIO17 | 11 | low (contact closed to GND) |
| Ruida K40 | Laser activity | GPIO27 | 13 | high (optocoupler off) |
| Ruida K40 | Harness sense | GPIO5 | 29 | low (shorted to GND pin 30 in the plug) |
| Galvo | Interlock | GPIO23 | 16 | low |
| Galvo | Laser activity | GPIO24 | 18 | high |
| Galvo | Harness sense | GPIO6 | 31 | low (shorted to GND pin 30 or 34 in the plug) |

Ground: pins 9, 14, 20, 25, 30, 34, 39. The pins can be changed in
`/etc/libreburn/hil-guard.toml`; set `enabled = false` under a machine to
leave it out.

### Wiring expectations

- **The Pi's GPIO is 3.3 V and not 5 V tolerant.** Nothing from the laser side
  connects to it directly; every signal crosses an optocoupler or a dry
  contact, and the laser and Pi grounds stay separate.
- **Interlock:** a dry contact that is closed only when the whole interlock
  chain is closed. Use a second, isolated contact on each interlock switch
  wired in series (or a relay driven by the existing chain). Do not tap the
  laser PSU's own interlock loop. Contact between the GPIO and a Pi GND pin;
  a 1 kΩ series resistor at the Pi end protects the input on a long cable.
- **Laser activity, K40:** an optocoupler LED (PC817 or similar) driven by the
  PSU's fire input, for example anode through 470 Ω to the PSU's +5 V and
  cathode to the `L` (fire, active low) line, so it lights whenever the
  controller fires. Check your PSU's pinout first. The transistor side goes
  collector to GPIO, emitter to Pi GND.
- **Laser activity, galvo:** tap the source's modulation/laser-on input on its
  DB25 (pin numbers differ between Raycus, JPT, MAX and others: check the
  source's manual). Pulses here are short, so use a fast optocoupler such as
  a 6N137 (5 V from the Pi's pin 2 or 4 on its output side, output open
  collector to the GPIO). Pick a signal the controller only asserts when the
  source would emit, not emission-enable, which may be on whenever a job is
  loaded.
- **Harness sense:** short the harness-sense pin to a GND pin inside each
  machine's plug, so unplugging the cable reads "harness missing".

### Commissioning and turning it on

1. With nothing connected, `gpioinfo | grep -E 'GPIO(5|6|17|23|24|27)'` should
   list the lines unused.
2. Turn the guard on: `sudo infra/lab/provision.sh hil-guard enable`.
3. Watch `sudo infra/lab/provision.sh hil-guard status` while you check each
   input by hand: plug and unplug each harness, open and close each
   interlock, and briefly short each laser-activity input on the machine side
   of the optocoupler (keys removed, as always). Each should change the
   matching field. Shorting a laser-activity input latches the guard, which
   is expected: clear it with `sudo systemctl restart libreburn-hil-guard`.
4. Run `hil.yml` by hand (workflow dispatch). It should print
   `interlock closed, no laser activity` and pass.

The latch can only be cleared by restarting the guard, which needs your sudo,
not the agent's.

To turn it off again: `sudo infra/lab/provision.sh hil-guard disable`.
