#!/usr/bin/env bash
# Run a long job with the machine held awake, then let it sleep again.
#
# Wrapping rather than arming and releasing around the job: the hold lives
# exactly as long as the command, so it can't outlive a crash or lapse while
# the job is still running. A sweep or a corpus benchmark takes longer than
# the idle-suspend timeout, and a suspend part-way through a measured run
# corrupts its timings silently.
#
# `sleep:idle`, not `sleep`: the idle path is the one that actually fires, and
# a `sleep`-only inhibitor does not cover it.
#
# Usage: scripts/keep-awake.sh <why> <command> [args...]
set -euo pipefail

if [ "$#" -lt 2 ]; then
  echo "usage: $0 <why> <command> [args...]" >&2
  exit 2
fi

why=$1
shift

# Probe before committing to it: a CI runner has systemd-inhibit on PATH but
# no session bus to take a lock on, and `exec` leaves nothing to catch the
# failure afterwards. Taking and dropping a lock on `true` costs nothing and
# is the only way to know the real thing would work.
if command -v systemd-inhibit >/dev/null 2>&1 \
    && systemd-inhibit --what=sleep:idle --who="vivace" --why="probe" true >/dev/null 2>&1; then
  exec systemd-inhibit --what=sleep:idle --who="vivace" --why="$why" -- "$@"
elif command -v caffeinate >/dev/null 2>&1; then
  # macOS: -i blocks idle sleep, -m keeps the disk awake for a job that is
  # mostly I/O.
  exec caffeinate -im "$@"
else
  echo "keep-awake: no systemd-inhibit or caffeinate, running unprotected" >&2
  exec "$@"
fi
