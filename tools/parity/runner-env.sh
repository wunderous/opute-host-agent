#!/usr/bin/env bash
# Print the host facts that host-read (get_host_info, get_host_capacity) and
# resource admission observe, so a diff seen only on one machine can be traced
# to the machine rather than to the implementations. Read-only.
set -u
section() { printf '\n== %s\n' "$1"; }
show() { for f in "$@"; do printf -- '-- %s\n' "$f"; cat "$f" 2>&1; done; }

section identity; uname -a; id; nproc; cat /proc/loadavg
section meminfo; head -5 /proc/meminfo
section psi; show /proc/pressure/cpu /proc/pressure/io /proc/pressure/memory
section cgroup; cat /proc/self/cgroup
cg=/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)
show /sys/fs/cgroup/cgroup.controllers "$cg/cgroup.controllers" "$(dirname "$cg")/cgroup.subtree_control" \
  "$cg/memory.max" "$cg/memory.current" "$cg/memory.events" "$cg/pids.max" "$cg/pids.current" "$cg/cpu.max"
section filesystems; df -PT / /tmp "${TMPDIR:-/tmp}" "$HOME" 2>&1; findmnt -no TARGET,SOURCE,FSTYPE,OPTIONS / /tmp 2>&1
section tools
for t in incus lxc zfs zpool systemctl dbus-daemon dbus-run-session busctl ss; do printf '%s: %s\n' "$t" "$(command -v "$t" || echo absent)"; done
section user-session; echo "XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-}"; systemctl --user is-system-running 2>&1
section ports; show /proc/sys/net/ipv4/ip_local_port_range; ss -ltn 2>&1
section limits; ulimit -a
