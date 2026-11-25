#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Run hardware-in-the-loop tests for one driver: hil/run.sh ruida|lmc
# Expects LIBREBURN_HIL_PROFILE to point at a profile under hil/profiles/.
set -euo pipefail
driver="${1:?usage: hil/run.sh ruida|lmc}"
profile="${LIBREBURN_HIL_PROFILE:?set LIBREBURN_HIL_PROFILE}"

case "$profile" in
  hil/profiles/*) ;;
  *) echo "LIBREBURN_HIL_PROFILE must be a file in hil/profiles/, got $profile" >&2; exit 1 ;;
esac

# Second safeguard: refuse to touch hardware unless every HIL profile caps power at zero.
cargo test -p libreburn-core --test repo_profiles --locked

cargo test -p "libreburn-driver-$driver" --locked -- --ignored hil_
