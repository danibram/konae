#!/bin/sh
set -eu
data="${KONAE_DATA_DIR:-/kernel}"
if [ "$(id -u)" = 0 ]; then
  owner="$(stat -c %u:%g "$data"/apps/*/ 2>/dev/null | head -n 1 || true)"
  case "$owner" in ""|0:*) owner=10001:10001 ;; esac
  chown "$owner" "$data" "$data/apps"
  export HOME="$data"
  if [ -n "${KONAE_FUNCTIONS_RUNNER:-}" ] && [ "${KONAE_FUNCTIONS_DEVELOPMENT:-0}" != "1" ]; then
    cgroup_mount=/sys/fs/cgroup
    cgroup_root=
    for group in "$cgroup_mount"/*; do
      if [ -f "$group/cgroup.procs" ] && grep -qx "$$" "$group/cgroup.procs"; then
        cgroup_root=$group
        break
      fi
    done
    test -n "$cgroup_root" || { echo "Konae functions could not find the container's delegated cgroup v2 root" >&2; exit 1; }
    test -f "$cgroup_root/cgroup.controllers" || { echo "Konae functions require a writable cgroup v2 subtree" >&2; exit 1; }
    grep -qw memory "$cgroup_root/cgroup.controllers" || { echo "Konae functions require the cgroup v2 memory controller" >&2; exit 1; }
    grep -qw pids "$cgroup_root/cgroup.controllers" || { echo "Konae functions require the cgroup v2 pids controller" >&2; exit 1; }
    functions_cgroup="$cgroup_root/konae-functions"
    mkdir "$cgroup_root/setup"
    printf '%s\n' "$$" > "$cgroup_root/setup/cgroup.procs"
    printf '%s\n' '+memory +pids' > "$cgroup_root/cgroup.subtree_control"
    mkdir -p "$functions_cgroup/kernel-manager"
    printf '%s\n' '+memory +pids' > "$functions_cgroup/cgroup.subtree_control"
    printf '%s\n' "$$" > "$functions_cgroup/kernel-manager/cgroup.procs"
    chown "$owner" "$functions_cgroup" "$functions_cgroup/cgroup.procs" \
      "$functions_cgroup/kernel-manager" "$functions_cgroup/kernel-manager/cgroup.procs"
    export KONAE_FUNCTIONS_CGROUP="$functions_cgroup"
    exec setpriv --reuid="${owner%%:*}" --regid="${owner##*:}" --clear-groups \
      --inh-caps=+setuid,+setgid --ambient-caps=+setuid,+setgid \
      -- /usr/local/lib/konae/kernel "$@"
  fi
  exec setpriv --reuid="${owner%%:*}" --regid="${owner##*:}" --clear-groups --inh-caps=-all -- /usr/local/lib/konae/kernel "$@"
fi
exec /usr/local/lib/konae/kernel "$@"
