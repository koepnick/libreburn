#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Provision libreburn lab machines. Idempotent: safe to run again after a change.
# Read docs/lab-setup.md first. Run from a checkout of the repository, as root.
#
#   sudo infra/lab/provision.sh host --role workstation|pi [options]
#   sudo infra/lab/provision.sh runner --role workstation|pi --version X.Y.Z --sha256 HEX
#   sudo infra/lab/provision.sh hil-guard enable|disable|status
#
# host options:
#   --agent-user NAME      local user that runs the Claude CLI (default: claude)
#   --agent-hardware       also let the agent user open the laser devices (off by default)
#   --agent-docker         give the agent user its own rootless Docker (off by default)
#   --firewall             install the host firewall (inbound SSH only from --ssh-allow)
#   --ssh-allow CIDRS      comma-separated (default: 10.0.0.0/8,172.16.0.0/12,192.168.0.0/16)
#   --ruida-net CIDR       Pi only: the Ruida's isolated segment; only root and the
#                          HIL runner may reach it, and nothing is routed to or from it
#   --ruida-host IP        Pi only: the board's address, passed to HIL jobs
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FILES="$REPO_ROOT/infra/lab/files"
RUNNER_BASE=/var/lib/libreburn-runners
ETC=/etc/libreburn

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run as root (sudo $0 ...)"

runner_user_for() {
  case "$1" in
    workstation) echo ghr-arm64 ;;
    pi) echo ghr-hil ;;
    *) die "--role must be workstation or pi" ;;
  esac
}

runner_labels_for() {
  case "$1" in
    workstation) echo arm64-cross ;;
    pi) echo hil-ruida,hil-galvo,pi5 ;;
  esac
}

ensure_system_user() { # name home
  if ! id "$1" >/dev/null 2>&1; then
    useradd --system --home-dir "$2" --create-home --shell /usr/sbin/nologin --user-group "$1"
  fi
  install -d -m 0700 -o "$1" -g "$1" "$2"
  # Runner users must never be root-equivalent.
  for g in sudo adm docker; do
    if id -nG "$1" | tr ' ' '\n' | grep -qx "$g"; then
      gpasswd -d "$1" "$g" >/dev/null
      warn "removed $1 from group $g"
    fi
  done
}

setup_rootless_docker() { # user
  local user="$1" uid tool
  uid="$(id -u "$user")"
  tool="$(command -v dockerd-rootless-setuptool.sh || true)"
  [ -n "$tool" ] || tool="$(ls /usr/share/docker.io/contrib/dockerd-rootless-setuptool.sh 2>/dev/null || true)"
  if [ -z "$tool" ]; then
    warn "dockerd-rootless-setuptool.sh not found; install docker-ce-rootless-extras (Docker's apt repo) and re-run. Skipping rootless Docker for $user."
    return
  fi
  # Each user gets a distinct subordinate ID range.
  if ! grep -q "^$user:" /etc/subuid; then
    local start=$((100000 + 65536 * (uid % 1000)))
    usermod --add-subuids "$start-$((start + 65535))" --add-subgids "$start-$((start + 65535))" "$user"
  fi
  loginctl enable-linger "$user"
  for _ in $(seq 20); do [ -d "/run/user/$uid" ] && break; sleep 0.5; done
  if [ ! -S "/run/user/$uid/docker.sock" ]; then
    say "Rootless Docker for $user"
    runuser -u "$user" -- env HOME="$(getent passwd "$user" | cut -d: -f6)" \
      XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus" \
      "$tool" install --skip-iptables
  fi
}

# ---------------------------------------------------------------------------- host

cmd_host() {
  local role="" agent=claude agent_hw=0 agent_docker=0 firewall=0
  local ssh_allow="10.0.0.0/8,172.16.0.0/12,192.168.0.0/16" ruida_net="" ruida_host=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --role) role="$2"; shift 2 ;;
      --agent-user) agent="$2"; shift 2 ;;
      --agent-hardware) agent_hw=1; shift ;;
      --agent-docker) agent_docker=1; shift ;;
      --firewall) firewall=1; shift ;;
      --ssh-allow) ssh_allow="$2"; shift 2 ;;
      --ruida-net) ruida_net="$2"; shift 2 ;;
      --ruida-host) ruida_host="$2"; shift 2 ;;
      *) die "unknown option $1" ;;
    esac
  done
  local runner arch
  runner="$(runner_user_for "$role")"
  arch="$(dpkg --print-architecture)"
  case "$role:$arch" in
    workstation:amd64 | pi:arm64) ;;
    *) die "role $role expects $([ "$role" = pi ] && echo arm64 || echo amd64), this is $arch" ;;
  esac

  say "Packages"
  apt-get update
  local pkgs=(ca-certificates curl git jq unattended-upgrades apt-listchanges
    build-essential pkg-config python3 nftables uidmap dbus-user-session)
  case "$role" in
    workstation) pkgs+=(gcc-aarch64-linux-gnu libc6-dev-arm64-cross qemu-user qemu-user-binfmt
      libusb-1.0-0-dev libudev-dev slirp4netns passt) ;;
    pi) pkgs+=(gpiod python3-libgpiod libusb-1.0-0-dev libudev-dev) ;;
  esac
  DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${pkgs[@]}" \
    || die "package install failed (names differ by release; edit pkgs in $0)"
  dpkg-reconfigure -f noninteractive unattended-upgrades >/dev/null

  say "Runner user $runner"
  ensure_system_user "$runner" "$RUNNER_BASE/$runner"
  install -d -m 0750 -o root -g "$runner" "$ETC/runners"
  install -d -m 0755 "$ETC"
  local envfile="$ETC/runners/$runner.env"
  {
    echo "# Managed by infra/lab/provision.sh; edit and re-run, or edit and restart the runner."
    case "$role" in
      workstation)
        echo "CARGO_BUILD_JOBS=$(( $(nproc) > 2 ? $(nproc) - 1 : 1 ))"
        echo "DOCKER_HOST=unix:///run/user/$(id -u "$runner")/docker.sock" ;;
      pi)
        echo "CARGO_BUILD_JOBS=2"
        [ -n "$ruida_host" ] && echo "LIBREBURN_RUIDA_HOST=$ruida_host" ;;
    esac
  } >"$envfile.new"
  if [ -f "$envfile" ] && [ "$role" = pi ] && [ -z "$ruida_host" ]; then
    grep '^LIBREBURN_RUIDA_HOST=' "$envfile" >>"$envfile.new" || true
  fi
  install -m 0640 -o root -g "$runner" "$envfile.new" "$envfile"; rm -f "$envfile.new"

  install -m 0644 "$FILES/libreburn-runner@.service" /etc/systemd/system/libreburn-runner@.service
  local dropin="/etc/systemd/system/libreburn-runner@$runner.service.d"
  install -d "$dropin"
  case "$role" in
    workstation)
      setup_rootless_docker "$runner"
      cat >"$dropin/role.conf" <<EOF
# Managed by infra/lab/provision.sh
[Unit]
Wants=user@$(id -u "$runner").service
After=user@$(id -u "$runner").service
[Service]
PrivateDevices=yes
BindPaths=/run/user/$(id -u "$runner")
EOF
      ;;
    pi)
      # Laser access through groups and udev, never root.
      usermod -aG dialout,plugdev "$runner"
      local mem_kb; mem_kb="$(awk '/MemTotal/ {print $2}' /proc/meminfo)"
      cat >"$dropin/role.conf" <<EOF
# Managed by infra/lab/provision.sh
[Service]
# Keep a runaway build from taking the Pi down; the OOM killer picks the job.
MemoryMax=$(( mem_kb * 80 / 100 ))K
EOF
      install -m 0644 "$REPO_ROOT/packaging/udev/70-libreburn.rules" /etc/udev/rules.d/70-libreburn.rules
      udevadm control --reload-rules && udevadm trigger --subsystem-match=usb --subsystem-match=tty || true
      say "HIL guard (installed, off until enabled)"
      install -D -m 0644 "$REPO_ROOT/hil/guard/libreburn_hil_guard.py" /usr/local/lib/libreburn/libreburn_hil_guard.py
      install -m 0644 "$FILES/libreburn-hil-guard.service" /etc/systemd/system/libreburn-hil-guard.service
      [ -f "$ETC/hil-guard.toml" ] || install -m 0644 "$FILES/hil-guard.toml" "$ETC/hil-guard.toml"
      ;;
  esac

  say "Agent user $agent"
  if ! id "$agent" >/dev/null 2>&1; then
    adduser --disabled-password --comment "Claude CLI agent" "$agent" >/dev/null 2>&1 \
      || adduser --disabled-password --gecos "Claude CLI agent" "$agent"
  fi
  for g in sudo docker dialout plugdev gpio; do
    if id -nG "$agent" | tr ' ' '\n' | grep -qx "$g"; then
      gpasswd -d "$agent" "$g" >/dev/null
      warn "removed $agent from group $g (see docs/lab-setup.md to widen access)"
    fi
  done
  usermod -aG systemd-journal "$agent"
  if [ "$agent_hw" = 1 ]; then
    usermod -aG dialout,plugdev "$agent"
    getent group gpio >/dev/null && usermod -aG gpio "$agent"
  fi
  # Other users cannot read the runner's files, tokens or env.
  chmod 0700 "$RUNNER_BASE/$runner"
  install -m 0755 "$FILES/lb-apt" /usr/local/sbin/lb-apt
  local sudoers=/etc/sudoers.d/50-libreburn-agent tmp
  tmp="$(mktemp)"
  cat >"$tmp" <<EOF
# Managed by infra/lab/provision.sh. Scoped, passwordless sudo for the Claude CLI
# agent user. Widening it: docs/lab-setup.md#widening-the-agents-sudo
Cmnd_Alias LB_PKG = /usr/local/sbin/lb-apt
Cmnd_Alias LB_RUNNER = /usr/bin/systemctl restart libreburn-runner@$runner.service, \\
                       /usr/bin/systemctl stop libreburn-runner@$runner.service, \\
                       /usr/bin/systemctl start libreburn-runner@$runner.service
Defaults:$agent env_reset, !setenv, use_pty, log_output
$agent ALL=(root) NOPASSWD: LB_PKG, LB_RUNNER
EOF
  visudo -cf "$tmp" >/dev/null || { rm -f "$tmp"; die "generated sudoers failed visudo -c"; }
  install -m 0440 "$tmp" "$sudoers"; rm -f "$tmp"
  if [ "$agent_docker" = 1 ]; then
    setup_rootless_docker "$agent"
  fi

  say "SSH"
  local admin="${SUDO_USER:-}"
  local admin_home; admin_home="$(getent passwd "$admin" | cut -d: -f6 || true)"
  if [ -n "$admin" ] && [ -s "$admin_home/.ssh/authorized_keys" ]; then
    cat >/etc/ssh/sshd_config.d/50-libreburn.conf <<'EOF'
# Managed by infra/lab/provision.sh
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
EOF
    if sshd -t; then systemctl reload ssh 2>/dev/null || systemctl reload sshd; else
      rm -f /etc/ssh/sshd_config.d/50-libreburn.conf; warn "sshd -t failed; left SSH unchanged"; fi
  else
    warn "no ~/.ssh/authorized_keys for '${admin:-?}'; leaving password SSH on so you are not locked out"
  fi

  if [ "$firewall" = 1 ]; then
    say "Firewall"
    local runner_uid fwd="" out=""
    runner_uid="$(id -u "$runner")"
    if [ -n "$ruida_net" ]; then
      fwd="    ip daddr $ruida_net drop"$'\n'"    ip saddr $ruida_net drop"
      out="    ip daddr $ruida_net meta skuid != { 0, $runner_uid } reject"
    fi
    sed -e "s#@SSH_ALLOW@#${ssh_allow//,/, }#" \
        -e "s#@RUIDA_FWD@#${fwd//$'\n'/\\n}#" \
        -e "s#@RUIDA_OUT@#$out#" \
        "$FILES/firewall.nft.in" >"$ETC/firewall.nft"
    nft -c -f "$ETC/firewall.nft" || die "firewall rules failed nft -c; not applied"
    cat >/etc/systemd/system/libreburn-firewall.service <<EOF
# Managed by infra/lab/provision.sh
[Unit]
Description=libreburn host firewall
Before=network-pre.target
Wants=network-pre.target
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/sbin/nft -f $ETC/firewall.nft
ExecStop=/usr/sbin/nft delete table inet libreburn
[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    systemctl enable libreburn-firewall.service >/dev/null
    systemctl restart libreburn-firewall.service
  fi

  systemctl daemon-reload
  say "Done. Next: register the runner (docs/lab-setup.md#register-the-runner)"
}

# -------------------------------------------------------------------------- runner

cmd_runner() {
  local role="" version="" sha=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --role) role="$2"; shift 2 ;;
      --version) version="${2#v}"; shift 2 ;;
      --sha256) sha="$2"; shift 2 ;;
      *) die "unknown option $1" ;;
    esac
  done
  [ -n "$version" ] && [ -n "$sha" ] || die "--version and --sha256 are required (from the actions/runner release notes)"
  local user labels home dir arch name
  user="$(runner_user_for "$role")"
  labels="$(runner_labels_for "$role")"
  id "$user" >/dev/null 2>&1 || die "run '$0 host --role $role' first"
  home="$RUNNER_BASE/$user"; dir="$home/runner"
  case "$(dpkg --print-architecture)" in amd64) arch=x64 ;; arm64) arch=arm64 ;; *) die "unsupported arch" ;; esac
  name="$(hostname -s)-$user"

  if [ ! -x "$dir/run.sh" ]; then
    say "Downloading actions-runner $version ($arch)"
    local tgz="$home/actions-runner.tar.gz"
    runuser -u "$user" -- curl -fsSL -o "$tgz" \
      "https://github.com/actions/runner/releases/download/v$version/actions-runner-linux-$arch-$version.tar.gz"
    echo "$sha  $tgz" | sha256sum -c - || { rm -f "$tgz"; die "checksum mismatch"; }
    runuser -u "$user" -- mkdir -p "$dir"
    runuser -u "$user" -- tar -xzf "$tgz" -C "$dir"
    rm -f "$tgz"
    say "Runner dependencies (.NET runtime libraries)"
    "$dir/bin/installdependencies.sh"
  fi

  if [ ! -f "$dir/.runner" ]; then
    local token
    echo "Create a registration token: GitHub > koepnick/libreburn > Settings > Actions > Runners > New self-hosted runner."
    read -r -s -p "Registration token (not echoed): " token; echo
    [ -n "$token" ] || die "no token"
    say "Registering $name with labels self-hosted,$labels"
    (cd "$dir" && runuser -u "$user" -- ./config.sh --unattended --replace \
      --url https://github.com/koepnick/libreburn --token "$token" \
      --name "$name" --labels "$labels" --work _work)
  fi

  systemctl daemon-reload
  systemctl enable --now "libreburn-runner@$user.service"
  systemctl --no-pager --lines=5 status "libreburn-runner@$user.service" || true
  say "Runner $name is up. Set the repository variable(s) in docs/lab-setup.md to route jobs to it."
}

# ----------------------------------------------------------------------- hil-guard

cmd_hil_guard() {
  local conf="$ETC/hil-guard.toml" dropin=/etc/systemd/system/libreburn-runner@ghr-hil.service.d/guard.conf
  [ -f "$conf" ] || die "run '$0 host --role pi' first"
  case "${1:-}" in
    enable)
      install -D -m 0644 "$REPO_ROOT/hil/guard/libreburn_hil_guard.py" /usr/local/lib/libreburn/libreburn_hil_guard.py
      sed -i 's/^enabled = false/enabled = true/' "$conf"
      cat >"$dropin" <<'EOF'
# Managed by infra/lab/provision.sh: the HIL runner only runs while the guard does.
[Unit]
BindsTo=libreburn-hil-guard.service
After=libreburn-hil-guard.service
EOF
      systemctl daemon-reload
      systemctl enable libreburn-hil-guard.service >/dev/null
      systemctl restart libreburn-hil-guard.service
      say "Guard on. It starts the runner once every enabled machine reads safe for 5 s."
      ;;
    disable)
      sed -i 's/^enabled = true/enabled = false/' "$conf"
      rm -f "$dropin"
      systemctl disable --now libreburn-hil-guard.service 2>/dev/null || true
      systemctl daemon-reload
      say "Guard off. HIL jobs run on the zero power cap and removed keys alone."
      ;;
    status)
      python3 -I /usr/local/lib/libreburn/libreburn_hil_guard.py status ;;
    *) die "usage: $0 hil-guard enable|disable|status" ;;
  esac
}

case "${1:-}" in
  host) shift; cmd_host "$@" ;;
  runner) shift; cmd_runner "$@" ;;
  hil-guard) shift; cmd_hil_guard "$@" ;;
  *) sed -n '3,19p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
