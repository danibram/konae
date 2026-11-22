#!/bin/sh
set -eu

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
cleanup() {
  if [ -f "$scratch/exported/compose.yaml" ]; then
    (cd "$scratch/exported" && KONAE_PORT="$port" docker compose down -v --remove-orphans >/dev/null 2>&1) || true
  fi
  docker image rm "$tag-kernel" "$tag-konae" >/dev/null 2>&1 || true
  rm -rf "$scratch"
}
trap cleanup EXIT HUP INT TERM

runner_dir="$scratch/runner"
sh "$root/scripts/build-functions-runner.sh" "$arch" "$runner_dir"
sh "$root/scripts/build-functions-images.sh" "$runner_dir" "$tag"

mkdir "$scratch/source" "$scratch/exported"
bun "$root/scripts/konae.ts" init --dir "$scratch/source" --name "Export function check"
bun -e 'const fs = require("node:fs"); const p = process.argv[1]; const s = JSON.parse(fs.readFileSync(p, "utf8")); s.slug = "export-functions-check"; s.visibility = "public"; s.modules = { ...(s.modules || {}), functions: { on: true } }; fs.writeFileSync(p, JSON.stringify(s, null, 2) + "\n");' "$scratch/source/konae.json"
mkdir -p "$scratch/source/app/functions"
cat > "$scratch/source/app/functions.json" <<'EOF'
{"version":1,"functions":{"hello":{"entry":"functions/hello.js","mount":"/api/hello","methods":["GET"],"access":"anonymous","actions":[]}}}
EOF
cat > "$scratch/source/app/functions/hello.js" <<'EOF'
export function handle() {
  return Response.json({ message: "exported function answered" });
}
EOF
bun "$root/scripts/konae.ts" export --dir "$scratch/source" --out "$scratch/exported"
sed "s|^KONAE_KERNEL_IMAGE=.*|KONAE_KERNEL_IMAGE=$tag-kernel|" "$scratch/exported/.env.example" > "$scratch/exported/.env"

if ! (cd "$scratch/exported" && KONAE_PORT="$port" docker compose up -d); then
  (cd "$scratch/exported" && KONAE_PORT="$port" docker compose logs kernel) >&2 || true
  exit 1
fi
healthy=0
for attempt in $(seq 1 60); do
  if curl --fail --silent "http://127.0.0.1:$port/_konae/health" >/dev/null; then healthy=1; break; fi
  sleep 1
done
if [ "$healthy" -ne 1 ]; then
  (cd "$scratch/exported" && KONAE_PORT="$port" docker compose logs kernel) >&2
  echo "Exported Kernel did not become healthy" >&2
  exit 1
fi
response=$(curl --silent --show-error -H 'Host: export-functions-check.localhost' -w '\n%{http_code}' "http://127.0.0.1:$port/api/hello")
case "$response" in
  *'"message":"exported function answered"'*) printf 'exported_compose_function=passed\nresponse=%s\n' "$response" ;;
  *)
    echo "Unexpected function response: $response" >&2
    (cd "$scratch/exported" && KONAE_PORT="$port" docker compose logs kernel) >&2
    exit 1
    ;;
esac
cat > "$scratch/deny-landlock.json" <<'EOF'
{"defaultAction":"SCMP_ACT_ALLOW","architectures":["SCMP_ARCH_AARCH64","SCMP_ARCH_X86_64"],"syscalls":[{"names":["landlock_create_ruleset"],"action":"SCMP_ACT_ERRNO","errnoRet":1}]}
EOF
cat > "$scratch/exported/compose.override.yaml" <<EOF
services:
  kernel:
    security_opt:
      - seccomp=$scratch/deny-landlock.json
EOF
(cd "$scratch/exported" && KONAE_PORT="$port" docker compose up -d --force-recreate kernel >/dev/null)
denied=""
for attempt in $(seq 1 30); do
  denied=$(curl --silent --show-error -H 'Host: export-functions-check.localhost' -w '\n%{http_code}' "http://127.0.0.1:$port/api/hello" 2>/dev/null || true)
  case "$denied" in *sandbox_unavailable*) break ;; esac
  sleep 1
done
case "$denied" in
  *sandbox_unavailable*) printf 'landlock_denied_function=passed\nresponse=%s\n' "$denied" ;;
  *) echo "The function did not report sandbox_unavailable when Landlock was denied: $denied" >&2; exit 1 ;;
esac
