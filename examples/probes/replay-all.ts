// Runs every probe replay in turn, each on its own port, and exits non-zero if
// any fails. Each probe starts and removes its own Kernel. The probes back the
// cells of docs/research/capability-matrix.md that no showcase replay proves.
//
//   bun run examples/probes/replay-all.ts
//   KONAE_PROBE_BASE_PORT=23000 bun run examples/probes/replay-all.ts
//
// KONAE_BIN runs an already built konae binary instead of compiling one.
import { readdirSync } from "node:fs";
import path from "node:path";

const root = import.meta.dir;
const base = Number(process.env.KONAE_PROBE_BASE_PORT ?? 23000);
const probes = readdirSync(root, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name).sort();
const failed: string[] = [];
for (const [index, name] of probes.entries()) {
  const port = base + index * 10;
  const started = Date.now();
  const run = Bun.spawnSync(["bun", "run", path.join(root, name, "replay.ts")], {
    env: { ...process.env, KONAE_PROBE_PORT: String(port) },
    stdout: "pipe",
    stderr: "pipe",
  });
  const seconds = ((Date.now() - started) / 1000).toFixed(1);
  const passes = new TextDecoder().decode(run.stdout).split("\n").filter((line) => line.startsWith("PASS ")).length;
  if (run.exitCode === 0) {
    console.log(`PASS ${name} (${passes} checks, ${seconds}s, port ${port})`);
  } else {
    failed.push(name);
    console.log(`FAIL ${name} (${seconds}s, port ${port})`);
    console.log(new TextDecoder().decode(run.stdout).split("\n").slice(-20).join("\n"));
    console.log(new TextDecoder().decode(run.stderr).split("\n").slice(-20).join("\n"));
  }
}
if (failed.length) {
  console.log(`Failed: ${failed.join(", ")}`);
  process.exit(1);
}
console.log(`All ${probes.length} probe replays passed.`);
