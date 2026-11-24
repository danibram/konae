import assert from "node:assert/strict";
import { mkdir, symlink } from "node:fs/promises";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { cases, settings } from "../node-compat/cases.mjs";

const root = resolve(process.argv[2] ?? "/tmp/konae-functions-node-compat-design-spike");
const kernel = resolve(process.env.KONAE_KERNEL_BIN ?? "kernel-rs/target/debug/kernel");
const runner = resolve(process.env.KONAE_FUNCTIONS_RUNNER ?? "kernel-rs/functions-runner/target/release/konae-functions-runner");
const mode = process.argv[3] ?? "baseline";
assert.ok(["baseline", "full"].includes(mode));

async function freePort() {
  const server = createServer();
  await new Promise((ready) => server.listen(0, "127.0.0.1", ready));
  const { port } = server.address();
  await new Promise((closed) => server.close(closed));
  return port;
}

for (const door of ["cli", "editor"]) {
  const folder = join(root, door);
  const port = String(await freePort());
  const origin = `http://127.0.0.1:${port}`;
  const data = join(root, "editor-kernel");
  const env = {
    ...process.env,
    KONAE_KERNEL_BIN: kernel,
    KONAE_FUNCTIONS_RUNNER: runner,
    KONAE_FUNCTIONS_DEVELOPMENT: "1",
  };
  let command = [kernel];
  if (door === "cli") {
    command = [process.execPath, "scripts/konae.ts", "dev", "--dir", folder, "--port", port, "--events-port", String(await freePort())];
  } else {
    await mkdir(join(data, "apps"), { recursive: true });
    await symlink(folder, join(data, "apps", settings.id)).catch((error) => {
      if (error.code !== "EEXIST") throw error;
    });
    Object.assign(env, {
      KONAE_DATA_DIR: data, KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: port,
      KONAE_HOST_SUFFIX: "localhost", KONAE_PUBLIC_ORIGIN: `http://localhost:${port}`,
      KONAE_EXECUTION_MODE: "test", KONAE_MAIL_TRANSPORT: "development",
    });
  }

  const child = Bun.spawn(command, { env, stdout: "pipe", stderr: "pipe" });
  const stdout = new Response(child.stdout).text();
  const stderr = new Response(child.stderr).text();
  try {
    let ready = false;
    for (let attempt = 0; attempt < 4800 && child.exitCode === null; attempt++) {
      try {
        const response = await fetch(origin + "/_konae/health");
        if (response.ok) { ready = true; break; }
      } catch {}
      await Bun.sleep(100);
    }
    assert.ok(ready, `${door} Kernel did not start`);
    console.log(`${door.toUpperCase()} ${command.join(" ")}`);
    for (const [name, probe] of Object.entries(cases)) {
      if (probe.refused) continue;
      const path = "/api/" + name;
      try {
        const response = await fetch(origin + path, { headers: { Host: `${settings.slug}.localhost:${port}` } });
        const body = await response.json();
        console.log(`GET ${path} -> ${response.status} ${JSON.stringify(body)}`);
        if (mode === "baseline" && probe.runnerGap) {
          assert.equal(response.status, 500);
          assert.equal(body.code, "handler_error");
        } else {
          assert.equal(response.status, 200);
          assert.deepEqual(body, probe.expected);
        }
      } catch (error) {
        process.exitCode = 1;
        console.error(`FAIL ${door} GET ${path}: ${error.message}`);
      }
    }
  } catch (error) {
    process.exitCode = 1;
    console.error(error.message);
  } finally {
    child.kill();
    await child.exited;
    const [out, err] = await Promise.all([stdout, stderr]);
    if (process.exitCode) console.error(out, err);
  }
}
