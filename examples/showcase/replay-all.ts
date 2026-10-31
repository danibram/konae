// Runs every showcase replay in turn, each on its own port, and exits non-zero
// if any fails. Each replay starts and removes its own Kernel.
//
//   bun run examples/showcase/replay-all.ts
//   KONAE_SHOWCASE_BASE_PORT=22000 bun run examples/showcase/replay-all.ts
import { readdirSync } from "node:fs";
import path from "node:path";

const root = import.meta.dir;
const base = Number(process.env.KONAE_SHOWCASE_BASE_PORT ?? 22000);
const apps = readdirSync(root, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name).sort();
const failed: string[] = [];
for (const [index, app] of apps.entries()) {
  const port = base + index * 10;
  const started = Date.now();
  const run = Bun.spawnSync(["bun", "run", path.join(root, app, "replay.ts")], {
    env: { ...process.env, KONAE_SHOWCASE_PORT: String(port) },
    stdout: "pipe",
    stderr: "pipe",
  });
  const seconds = ((Date.now() - started) / 1000).toFixed(1);
  if (run.exitCode === 0) {
    console.log(`PASS ${app} (${seconds}s, port ${port})`);
  } else {
    failed.push(app);
    console.log(`FAIL ${app} (${seconds}s, port ${port})`);
    console.log(new TextDecoder().decode(run.stdout).split("\n").slice(-20).join("\n"));
    console.log(new TextDecoder().decode(run.stderr).split("\n").slice(-20).join("\n"));
  }
}
if (failed.length) {
  console.log(`Failed: ${failed.join(", ")}`);
  process.exit(1);
}
console.log(`All ${apps.length} showcase replays passed.`);
