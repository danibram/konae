#!/bin/sh
set -eu
data="${KONAE_DATA_DIR:-/kernel}"
export KONAE_FUNCTIONS_RUNNER_KIND="${KONAE_FUNCTIONS_RUNNER_KIND-wasm}"
case "$KONAE_FUNCTIONS_RUNNER_KIND" in
  wasm)
    export KONAE_FUNCTIONS_RUNNER="${KONAE_FUNCTIONS_RUNNER:-/usr/local/lib/konae/functions-runner-wasm}"
    export KONAE_FUNCTIONS_WASM_CACHE="${KONAE_FUNCTIONS_WASM_CACHE:-$data/.konae/wasm-cache}"
    export KONAE_FUNCTIONS_PREPARER="${KONAE_FUNCTIONS_PREPARER:-/usr/local/lib/konae/functions-prepare}"
    ;;
  deno)
    export KONAE_FUNCTIONS_RUNNER="${KONAE_FUNCTIONS_RUNNER:-/usr/local/lib/konae/functions-runner-deno}"
    ;;
esac
if [ "$(id -u)" = 0 ]; then
  owner="$(stat -c %u:%g "$data"/apps/*/ 2>/dev/null | head -n 1 || true)"
  case "$owner" in ""|0:*) owner=10001:10001 ;; esac
  chown "$owner" "$data" "$data/apps"
  chmod 0700 "$data"
  export HOME="$data"
  if [ "$KONAE_FUNCTIONS_RUNNER_KIND" = wasm ]; then
    cache=$KONAE_FUNCTIONS_WASM_CACHE
    mkdir -p "$cache"
    if [ "$cache" = "$data/.konae/wasm-cache" ]; then
      chown "$owner" "$data/.konae"
    fi
    chown "$owner" "$cache"
    chmod 0700 "$cache"
  fi
  if [ -n "${KONAE_FUNCTIONS_RUNNER:-}" ] && [ "${KONAE_FUNCTIONS_DEVELOPMENT:-0}" != "1" ]; then
    if [ "$KONAE_FUNCTIONS_RUNNER_KIND" = deno ] && [ -z "${KONAE_FUNCTIONS_CGROUP:-}" ]; then
      cgroup_mount=/sys/fs/cgroup
      # A private cgroup namespace puts this process at the namespace root. Docker mounts that
      # root read-only; with CAP_SYS_ADMIN the entrypoint remounts it writable, and setpriv drops
      # the capability before the Kernel starts.
      if grep -qx "$$" "$cgroup_mount/cgroup.procs" 2>/dev/null; then
        cgroup_root=$cgroup_mount
        if ! touch "$cgroup_mount/.konae-write-check" 2>/dev/null; then
          mount -o remount,rw "$cgroup_mount" || { echo "Konae functions need a writable cgroup v2 root: grant CAP_SYS_ADMIN for the entrypoint (cap_add: SYS_ADMIN) and allow mounts (security_opt: apparmor=unconfined), or mount a writable delegated subtree" >&2; exit 1; }
        else
          rm -f "$cgroup_mount/.konae-write-check"
        fi
      else
        cgroup_root=
        for group in "$cgroup_mount"/*; do
          if [ -f "$group/cgroup.procs" ] && grep -qx "$$" "$group/cgroup.procs"; then
            cgroup_root=$group
            break
          fi
        done
      fi
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
    fi
    exec setpriv --reuid="${owner%%:*}" --regid="${owner##*:}" --clear-groups \
      --bounding-set=-all,+setuid,+setgid,+kill \
      --inh-caps=+setuid,+setgid,+kill --ambient-caps=+setuid,+setgid,+kill \
      -- /usr/local/lib/konae/kernel "$@"
  fi
  exec setpriv --reuid="${owner%%:*}" --regid="${owner##*:}" --clear-groups --inh-caps=-all -- /usr/local/lib/konae/kernel "$@"
fi
exec /usr/local/lib/konae/kernel "$@"
