#!/bin/sh
set -eu

runner_kind=${1:-wasm}
case "$runner_kind" in
  wasm|deno) ;;
  *) echo "Usage: $0 [wasm|deno]" >&2; exit 2 ;;
esac

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
root=$(CDPATH= cd "$script_dir/.." && pwd)
case "$(uname -m)" in
  arm64|aarch64) arch=arm64 ;;
  x86_64|amd64) arch=amd64 ;;
  *) echo "Unsupported host architecture: $(uname -m)" >&2; exit 2 ;;
esac
command -v bun >/dev/null 2>&1 || { echo "bun is required" >&2; exit 1; }
command -v docker >/dev/null 2>&1 || { echo "docker is required" >&2; exit 1; }

scratch=$(mktemp -d "$root/.konae-export-functions.XXXXXX")
tag="konae-export-check-$$"
port=${KONAE_EXPORT_CHECK_PORT:-18787}
COMPOSE_PROJECT_NAME="konae-exported-functions-$$"
export COMPOSE_PROJECT_NAME
compose() {
  (cd "$scratch/exported" && KONAE_PORT="$port" docker compose "$@")
}
request() {
  if [ "$runner_kind" = deno ]; then
    compose exec -T kernel curl "$@"
  else
    curl "$@"
  fi
}
cleanup() {
  if [ "${KONAE_KEEP_EXPORT_CHECK:-0}" = 1 ]; then
    printf 'export_check_files=%s\n' "$scratch" >&2
    return
  fi
  if [ -f "$scratch/exported/compose.yaml" ]; then
    compose down -v --remove-orphans >/dev/null 2>&1 || true
  fi
  docker image rm "$tag-kernel" "$tag-konae" >/dev/null 2>&1 || true
  rm -rf "$scratch"
}
trap cleanup EXIT HUP INT TERM

image=${KONAE_EXPORT_CHECK_IMAGE:-$tag-kernel}
if [ -n "${KONAE_EXPORT_CHECK_IMAGE:-}" ]; then
  docker image inspect "$image" >/dev/null
else
  runner_dir=${KONAE_FUNCTIONS_RUNNER_CONTEXT:-$scratch/runner}
  if [ -z "${KONAE_FUNCTIONS_RUNNER_CONTEXT:-}" ]; then
    sh "$root/scripts/build-functions-runner.sh" "$arch" "$runner_dir"
  fi
  sh "$root/scripts/build-functions-images.sh" "$runner_dir" "$tag"
fi

mkdir "$scratch/source" "$scratch/exported"
bun "$root/scripts/konae.ts" init --dir "$scratch/source" --name "Export function check"
bun -e 'const fs = require("node:fs"); const p = process.argv[1]; const s = JSON.parse(fs.readFileSync(p, "utf8")); s.slug = "export-functions-check"; s.visibility = "private"; s.signupPolicy = { mode: "open", allowlist: [] }; s.modules = { ...(s.modules || {}), auth: { ...(s.modules?.auth || {}), on: true, signupPolicy: { mode: "open", allowlist: [] } }, actions: { on: true }, functions: { on: true } }; fs.writeFileSync(p, JSON.stringify(s, null, 2) + "\n");' "$scratch/source/konae.json"
mkdir -p "$scratch/source/app/functions"
cat > "$scratch/source/app/schema.sql" <<'EOF'
CREATE TABLE IF NOT EXISTS count_state (id INTEGER PRIMARY KEY, n INTEGER NOT NULL);
EOF
cat > "$scratch/source/app/actions.json" <<'EOF'
{"version":1,"actions":{"count.read":{"kind":"query","access":"authenticated","statement":"SELECT n FROM count_state WHERE id=1 LIMIT 1","bindings":[]},"count.increment":{"kind":"mutation","access":"authenticated","statement":"INSERT INTO count_state(id,n) VALUES(1,1) ON CONFLICT(id) DO UPDATE SET n=n+1","bindings":[]}}}
EOF
cat > "$scratch/source/app/functions.json" <<'EOF'
{
  "version": 1,
  "functions": {
    "counter0": {
      "entry": "functions/counter.js",
      "mount": "/api/counter0",
      "methods": ["GET", "POST"],
      "access": "authenticated",
      "actions": ["count.read", "count.increment"]
    },
    "stall": {
      "entry": "functions/stall.js",
      "mount": "/api/stall",
      "methods": ["GET"],
      "access": "authenticated"
    }
  }
}
EOF
cat > "$scratch/source/app/functions/counter.js" <<'EOF'
export async function handle(request, context) {
  if (new URL(request.url).pathname.endsWith("/headers")) {
    return new Response("test", { headers: { "content-security-policy-report-only": "default-src 'none'" } });
  }
  if (request.method === "POST") {
    await request.text();
    await context.actions.call("count.increment", {});
  }
  const rows = await context.actions.call("count.read", {});
  return Response.json({ n: rows[0]?.n ?? 0 });
}
EOF
cat > "$scratch/source/app/functions/stall.js" <<'EOF'
export function handle() {
  while (true) {}
}
EOF
bun "$root/scripts/konae.ts" export --dir "$scratch/source" --out "$scratch/exported"
sed "s|^KONAE_KERNEL_IMAGE=.*|KONAE_KERNEL_IMAGE=$image|" "$scratch/exported/.env.example" > "$scratch/exported/.env"
request_url="http://127.0.0.1:$port"
response_dir="$scratch"
if [ "$runner_kind" = deno ]; then
  cat >> "$scratch/exported/.env" <<'EOF'
KONAE_FUNCTIONS_RUNNER=/usr/local/lib/konae/functions-runner-deno
KONAE_FUNCTIONS_RUNNER_KIND=deno
KONAE_FUNCTIONS_DEVELOPMENT=1
EOF
  cat > "$scratch/exported/compose.override.yaml" <<'EOF'
services:
  kernel:
    environment:
      KONAE_BIND: "127.0.0.1"
    volumes:
      - ..:/proof
EOF
  request_url="http://127.0.0.1:8787"
  response_dir=/proof
  if cgroup_result=$(compose run --rm --no-deps +    -e KONAE_FUNCTIONS_DEVELOPMENT=0 +    -e KONAE_FUNCTIONS_CGROUP=relative-test-path kernel 2>&1); then
    echo "The Kernel accepted a relative cgroup path" >&2
    exit 1
  fi
  printf '%s\n' "$cgroup_result" | grep -F 'KONAE_FUNCTIONS_CGROUP must be absolute' || {
    printf '%s\n' "$cgroup_result" >&2
    exit 1
  }
else
  sed -e '/^KONAE_FUNCTIONS_RUNNER=/d' -e '/^KONAE_FUNCTIONS_RUNNER_KIND=/d' -e '/^KONAE_FUNCTIONS_WASM_CACHE=/d' \
    "$scratch/exported/.env" > "$scratch/exported/.env.proof"
  mv "$scratch/exported/.env.proof" "$scratch/exported/.env"
  cat > "$scratch/preparer" <<'EOF'
#!/bin/sh
set -eu
if [ "$1" = prepare ]; then
  awk -v pid="$$" '
    /^Cap(Inh|Prm|Eff|Amb):/ && $2 !~ /^0+$/ { exit 1 }
    /^NoNewPrivs:/ { nnp = $2 }
    /^NSpgid:/ { pgid = $NF }
    END { if (nnp != 1 || pgid != pid) exit 1 }
  ' "/proc/$$/status"
  awk '/^(CapInh|CapPrm|CapEff|CapAmb|NoNewPrivs|NSpgid):/' "/proc/$$/status" > /proof/preparer.status
fi
exec /usr/local/lib/konae/functions-prepare "$@"
EOF
  chmod +x "$scratch/preparer"
  cat > "$scratch/exported/compose.override.yaml" <<'EOF'
services:
  kernel:
    environment:
      KONAE_FUNCTIONS_RUNNER:
      KONAE_FUNCTIONS_RUNNER_KIND:
      KONAE_FUNCTIONS_WASM_CACHE:
      KONAE_FUNCTIONS_PREPARER: /proof/preparer
    volumes:
      - ..:/proof
EOF
fi

start_export() {
  if ! compose up -d; then
    compose logs kernel >&2 || true
    return 1
  fi
  for attempt in $(seq 1 "$1"); do
    if request --fail --silent "$request_url/_konae/health" >/dev/null; then return 0; fi
    sleep 1
  done
  compose logs kernel >&2
  echo "Exported Kernel did not become healthy" >&2
  return 1
}
start_export 180
host=export-functions-check.localhost:$port
signup_status=$(request --silent --show-error --dump-header "$response_dir/signup.headers" -o "$response_dir/signup.body" -w '%{http_code}' -H "Host: $host" -H "Origin: http://$host" -H 'Content-Type: application/json' -d '{"email":"worker@example.test","password":"correct horse battery"}' "$request_url/_konae/auth/signup")
test "$signup_status" = 200 || {
  cat "$scratch/signup.body" >&2
  echo "Signup failed with HTTP $signup_status" >&2
  compose logs kernel >&2 || true
  exit 1
}
cookie=$(awk '/^[Ss]et-[Cc]ookie:/ {sub(/^[^:]*:[[:space:]]*/, ""); sub(/;.*/, ""); print; exit}' "$scratch/signup.headers")
test -n "$cookie" || { echo "Signup returned no session cookie" >&2; exit 1; }
get_result=$(request --silent --show-error -o "$response_dir/get.body" -w '%{http_code} %{time_total}' -H "Host: $host" -H "Cookie: $cookie" "$request_url/api/counter0")
get_status=${get_result%% *}
get_seconds=${get_result#* }
get=$(cat "$scratch/get.body")
test "$get_status" = 200 && test "$get" = '{"n":0}' || { echo "Unexpected authenticated GET ($get_status): $get" >&2; exit 1; }
post_status=$(request --silent --show-error -o "$response_dir/post.body" -w '%{http_code}' -H "Host: $host" -H "Origin: http://$host" -H "Cookie: $cookie" -H 'Idempotency-Key: export-check-1' -H 'Content-Type: application/json' -d '{}' "$request_url/api/counter0")
post=$(cat "$scratch/post.body")
test "$post_status" = 200 && test "$post" = '{"n":1}' || { echo "Unexpected authenticated POST ($post_status): $post" >&2; exit 1; }
printf 'runner_kind=%s\nsignup=HTTP_%s\nauthenticated_get=%s %s\nauthenticated_post=%s %s\ncold_get_seconds=%s\n' "$runner_kind" "$signup_status" "$get_status" "$get" "$post_status" "$post" "$get_seconds"
header_status=$(request --silent --show-error -o "$response_dir/headers.body" -w '%{http_code}' -H "Host: $host" -H "Cookie: $cookie" "$request_url/api/counter0/headers")
header_body=$(cat "$scratch/headers.body")
test "$header_status" = 500 && printf '%s' "$header_body" | bun -e 'if ((await Bun.stdin.json()).code !== "function.output-invalid") process.exit(1)' || {
  echo "Response header check returned HTTP $header_status: $header_body" >&2
  exit 1
}
printf 'response_header_check=HTTP_%s %s\n' "$header_status" "$header_body"
test "$(compose exec -T kernel stat -c %a /kernel)" = 700
worker_count() {
  compose exec -T kernel sh -c 'for status in /proc/[0-9]*/status; do tr "\000" "\n" < "${status%/status}/cmdline" 2>/dev/null | grep -qx -- "--function-worker" && echo x; done; true' | wc -l | tr -d ' '
}
workers_before=$(worker_count)
test "$workers_before" -ge 1 || { echo "Expected a warm function worker before the deadline probe" >&2; exit 1; }
stall_status=$(request --silent --show-error --max-time 4 -o "$response_dir/stall.body" -w '%{http_code}' -H "Host: $host" -H "Cookie: $cookie" "$request_url/api/stall" || true)
test "$stall_status" = 504 || { echo "Deadline probe returned HTTP $stall_status, expected 504" >&2; cat "$scratch/stall.body" >&2; exit 1; }
for attempt in $(seq 1 30); do
  workers=$(worker_count)
  [ "$workers" -le "$workers_before" ] && break
  sleep 0.1
done
test "$workers" -le "$workers_before" || { echo "Timed-out worker remained alive" >&2; exit 1; }
printf 'deadline_probe=HTTP_%s worker_count_after_reap=%s\n' "$stall_status" "$workers"
reuse_status=$(request --silent --show-error -o "$response_dir/reuse.body" -w '%{http_code}' -H "Host: $host" -H "Cookie: $cookie" "$request_url/api/counter0")
test "$reuse_status" = 200 && test "$(cat "$scratch/reuse.body")" = '{"n":1}' || { echo "A counter worker did not run after the deadline cleanup" >&2; exit 1; }
printf 'slot_reuse=HTTP_%s %s\n' "$reuse_status" "$(cat "$scratch/reuse.body")"
readback=$(compose exec -T kernel kernel sql 'SELECT n FROM count_state WHERE id=1 LIMIT 1;')
printf '%s\n' "$readback" | bun -e 'const result = await Bun.stdin.json(); if (result.rows?.length !== 1 || result.rows[0].n !== 1) throw Error(JSON.stringify(result));'
printf 'sqlite_readback=%s\n' "$readback"

if [ "$runner_kind" = deno ]; then
  exit 0
fi

test "$(compose exec -T kernel stat -c %a /kernel/.konae/wasm-cache)" = 700
printf 'preparer_security=\n'
cat "$scratch/preparer.status"
worker_report=$(compose exec -T kernel sh -ec '
  for status in /proc/[0-9]*/status; do
    pid=${status#/proc/}; pid=${pid%/status}
    tr "\000" "\n" < "/proc/$pid/cmdline" | grep -qx -- "--function-worker" || continue
    echo "worker_pid=$pid"
    awk "/^(Uid|Gid|CapEff|NoNewPrivs|Seccomp|Seccomp_filters):/ {print}" "$status"
    awk "/^VmSize:/ {print}" "$status"
    grep -E "Max core file size|Max open files|Max processes|Max file size|Max address space" "/proc/$pid/limits"
    exit 0
  done
  exit 1')
printf '%s\n' "$worker_report" | grep -Eq '^Uid:[[:space:]]+10002[[:space:]]+10002' || { echo "Worker did not use UID 10002" >&2; exit 1; }
printf '%s\n' "$worker_report" | grep -Eq '^Gid:[[:space:]]+10002[[:space:]]+10002' || { echo "Worker did not use GID 10002" >&2; exit 1; }
printf '%s\n' "$worker_report" | grep -Eq '^CapEff:[[:space:]]+0+$' || { echo "Worker has effective capabilities" >&2; exit 1; }
printf '%s\n' "$worker_report" | grep -Eq '^NoNewPrivs:[[:space:]]+1$' || { echo "Worker lacks no_new_privs" >&2; exit 1; }
printf '%s\n' "$worker_report" | grep -Eq '^Seccomp:[[:space:]]+2$' || { echo "Worker is outside Docker's default seccomp filter" >&2; exit 1; }
printf '%s\n' "$worker_report" | grep -Eq '^Max core file size[[:space:]]+0[[:space:]]+0' || { echo "Worker core rlimit is not zero" >&2; exit 1; }
printf '%s\n' "$worker_report" | grep -Eq '^Max open files[[:space:]]+64[[:space:]]+64' || { echo "Worker open-file rlimit is not 64" >&2; exit 1; }
printf '%s\n' "$worker_report" | grep -Eq '^Max processes[[:space:]]+64[[:space:]]+64' || { echo "Worker process rlimit is not 64" >&2; exit 1; }
worker_filters=$(printf '%s\n' "$worker_report" | awk '/^Seccomp_filters:/ {print $2}')
kernel_filters=$(compose exec -T kernel awk '/^Seccomp_filters:/ {print $2}' /proc/1/status)
test "$worker_filters" -gt "$kernel_filters" || { echo "Worker lacks its own seccomp filter" >&2; exit 1; }
printf '%s\n' "$worker_report" | grep -Eq '^Max file size[[:space:]]+0[[:space:]]+0' || { echo "Worker file-size rlimit is not zero" >&2; exit 1; }
printf '%s\n' "$worker_report" | grep -Eq '^Max address space[[:space:]]+25769803776[[:space:]]+25769803776[[:space:]]+bytes' || { echo "Worker address-space rlimit differs from the 24 GiB Wasmtime pool budget" >&2; exit 1; }
kernel_caps=$(compose exec -T kernel awk '/^CapEff:/ {print $2}' /proc/1/status)
test "$kernel_caps" = 00000000000000e0 || { echo "Unexpected Kernel effective capabilities: $kernel_caps" >&2; exit 1; }
container_id=$(compose ps -q kernel)
cap_add=$(docker inspect --format '{{json .HostConfig.CapAdd}}' "$container_id")
test "$cap_add" = '["CAP_KILL","CAP_SETGID","CAP_SETUID"]' || { echo "Unexpected Compose capability additions: $cap_add" >&2; exit 1; }
test "$(docker inspect --format '{{.HostConfig.Privileged}}' "$container_id")" = false
security_options=$(docker inspect --format '{{json .HostConfig.SecurityOpt}}' "$container_id")
case "$security_options" in
  '[]'|null) ;;
  *) echo "Compose overrides Docker security options: $security_options" >&2; exit 1 ;;
esac
apparmor_profile=$(docker inspect --format '{{.AppArmorProfile}}' "$container_id")
printf 'worker_security=\n%s\nhost_kernel=%s\nhost_uname=%s\nimage_digest=%s\nkernel_cap_eff=%s\ncompose_cap_add=%s\nsecurity_options=%s\napparmor_profile=%s\n' \
  "$worker_report" "$(docker info --format '{{.KernelVersion}}')" "$(uname -srmo)" \
  "$(docker inspect --format '{{.Image}}' "$container_id")" "$kernel_caps" "$cap_add" "$security_options" "$apparmor_profile"

# Each distinct bundle adds a native component to the worker's resident memory.
mkdir "$scratch/rss-source" "$scratch/rss-export"
bun "$root/scripts/konae.ts" init --dir "$scratch/rss-source" --name "Export RSS check"
bun -e '
  const fs = require("node:fs");
  const root = process.argv[1];
  const settingsPath = `${root}/konae.json`;
  const settings = JSON.parse(fs.readFileSync(settingsPath, "utf8"));
  settings.slug = "export-rss-check";
  settings.visibility = "public";
  settings.modules.functions = { on: true };
  fs.writeFileSync(settingsPath, JSON.stringify(settings));
  fs.mkdirSync(`${root}/app/functions`, { recursive: true });
  const functions = {};
  for (let i = 0; i < 12; i++) {
    const name = `probe${i}`;
    const entry = `functions/${name}.js`;
    fs.writeFileSync(`${root}/app/${entry}`, `export function handle() { return new Response("${name}"); }`);
    functions[name] = { entry, mount: `/api/${name}`, methods: ["GET"], access: "anonymous" };
  }
  fs.writeFileSync(`${root}/app/functions.json`, JSON.stringify({ version: 1, functions }));
' "$scratch/rss-source"
bun "$root/scripts/konae.ts" export --dir "$scratch/rss-source" --out "$scratch/rss-export"
rss_id=$(bun -e 'console.log((await Bun.file(process.argv[1]).json()).id)' "$scratch/rss-export/konae.json")
cat > "$scratch/exported/compose.override.yaml" <<EOF
services:
  kernel:
    environment:
      KONAE_FUNCTIONS_TIMEOUT_MS: "4000"
    volumes:
      - ../rss-export:/kernel/apps/$rss_id
EOF
compose stop kernel
start_export 480
rss_status=$(curl --silent --show-error -o "$scratch/rss.body" -w '%{http_code}' -H "Host: export-rss-check.localhost:$port" "http://127.0.0.1:$port/api/probe0")
rss_body=$(cat "$scratch/rss.body")
test "$rss_status" = 502 && printf '%s' "$rss_body" | bun -e 'if ((await Bun.stdin.json()).code !== "worker_memory_limit") process.exit(1)' || {
  echo "RSS probe returned HTTP $rss_status: $rss_body" >&2
  compose logs kernel >&2
  exit 1
}
test "$(worker_count)" = 0 || { echo "RSS-limited worker remained alive" >&2; exit 1; }
reuse_status=$(curl --silent --show-error -o "$scratch/reuse.body" -w '%{http_code}' -H "Host: $host" -H "Cookie: $cookie" "http://127.0.0.1:$port/api/counter0")
test "$reuse_status" = 200 && test "$(cat "$scratch/reuse.body")" = '{"n":1}' || { echo "Counter app failed after RSS cleanup" >&2; exit 1; }
printf 'rss_probe=HTTP_%s %s\nrss_worker_reaped=yes\nafter_rss=HTTP_%s %s\n' "$rss_status" "$rss_body" "$reuse_status" "$(cat "$scratch/reuse.body")"
awk -v seconds="$get_seconds" 'BEGIN { if (seconds >= 0.250) exit 1 }' || { echo "Prepared cold GET took ${get_seconds}s, over 250ms" >&2; exit 1; }
