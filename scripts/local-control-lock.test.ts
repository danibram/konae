import { afterEach, expect, test } from "bun:test";
import { chmod, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { initLocal, localKernel, resetData } from "./konae";

const roots: string[] = [];
afterEach(async () => {
  for (const root of roots.splice(0)) await rm(root, { recursive: true, force: true });
});

async function exists(file: string) {
  return stat(file).then(() => true, () => false);
}

async function waitFor(file: string, child?: ReturnType<typeof Bun.spawn>) {
  for (let attempt = 0; attempt < 200; attempt++) {
    if (await exists(file)) return;
    if (child?.exitCode !== null && child?.exitCode !== undefined) throw Error(`Child exited ${child.exitCode} before creating ${file}.`);
    await Bun.sleep(10);
  }
  throw Error(`Timed out waiting for ${file}.`);
}

test("local replies and schedules share a lock with reset and release it after errors", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-control-lock-"));
  roots.push(root);
  const settings = await initLocal(root, { name: "Control lock", template: "tasks" });
  const binary = path.join(root, "kernel-control-stub.cjs");
  const started = path.join(root, ".konae/kernel/control-started");
  const gate = path.join(root, ".konae/kernel/control-release");
  await writeFile(binary, `#!/usr/bin/env node
const fs = require("node:fs");
const args = process.argv.slice(2);
if (process.argv.includes("capabilities")) { console.log(JSON.stringify({executionTest:1})); process.exit(0); }
if (args.includes("reply") || args.includes("schedule")) {
  const action = args[1];
  const call = args[2] || "schedule";
  if (call === "fail") { process.stderr.write("controlled operator failure"); process.exit(1); }
  fs.writeFileSync(process.env.KONAE_DATA_DIR + "/control-started", action);
  const interval = setInterval(() => {
    if (fs.existsSync(process.env.KONAE_DATA_DIR + "/control-release")) {
      clearInterval(interval);
      process.stdout.write('{"completed":true}');
      process.exit(0);
    }
  }, 10);
}
`);
  await chmod(binary, 0o755);
  const capabilityProbe = Bun.spawn([binary, "capabilities"], { stdout: "pipe", stderr: "pipe" });
  const [probeCode, probeOutput, probeError] = await Promise.all([capabilityProbe.exited, new Response(capabilityProbe.stdout).text(), new Response(capabilityProbe.stderr).text()]);
  expect({ probeCode, probeOutput, probeError }).toEqual({ probeCode: 0, probeOutput: expect.stringContaining("executionTest"), probeError: "" });
  const previousBinary = process.env.KONAE_KERNEL_BIN;
  process.env.KONAE_KERNEL_BIN = binary;
  try {
    const kernel = await localKernel(root, settings, { build: false });
    await writeFile(path.join(root, ".konae/control.lock"), JSON.stringify({ pid: 999999999, at: Date.now() }));
    await expect(kernel.run(["test", "reply", "call-stale"], "{}")).rejects.toThrow("needs operator recovery");
    await rm(path.join(root, ".konae/control.lock"), { force: true });
    await mkdir(path.join(root, "data"), { recursive: true });
    await writeFile(path.join(root, "data/old-state"), "keep until reset starts");
    const resetCalls: string[] = [];
    const resetKernel = {
      data: kernel.data,
      async run(args: string[]) {
        resetCalls.push(args.join(" "));
        return JSON.stringify({ callsRetired: 0, mailRetired: 0 });
      },
    };

    for (const [action, call] of [["reply", "call-1"], ["schedule", "daily.run"]]) {
      await rm(started, { force: true });
      await rm(gate, { force: true });
      const runningControl = kernel.run(["test", action, call], action === "reply" ? "{}" : undefined);
      try {
        await waitFor(started);
        await expect(resetData(root, resetKernel, undefined)).rejects.toThrow("local test control or reset");
        expect(resetCalls).toEqual([]);
        expect(await readFile(path.join(root, "data/old-state"), "utf8")).toBe("keep until reset starts");
        expect(await exists(path.join(root, ".konae/lifecycle.lock"))).toBe(false);
      } finally {
        await writeFile(gate, "go");
        await runningControl;
      }
      expect(await exists(path.join(root, ".konae/control.lock"))).toBe(false);
    }

    let releaseReset!: () => void;
    let enteredReset!: () => void;
    const resetEntered = new Promise<void>((resolve) => { enteredReset = resolve; });
    const resetGate = new Promise<void>((resolve) => { releaseReset = resolve; });
    const heldResetCalls: string[] = [];
    const heldResetKernel = {
      data: kernel.data,
      async run(args: string[]) {
        heldResetCalls.push(args.join(" "));
        if (args[0] === "test" && args[1] === "reset-effects") {
          enteredReset();
          await resetGate;
        }
        return JSON.stringify({ callsRetired: 0, mailRetired: 0 });
      },
    };
    const resetting = resetData(root, heldResetKernel, undefined);
    try {
      await Promise.race([resetEntered, resetting.then(() => { throw Error("Reset finished before its test barrier."); })]);
      await rm(started, { force: true });
      expect(await exists(path.join(root, ".konae/control.lock"))).toBe(true);
      await expect(kernel.run(["test", "calls"])).resolves.toBe("");
      await expect(kernel.run(["test", "reply", "call-during-reset"], "{}"))
        .rejects.toThrow("local test control or reset");
      await expect(kernel.run(["test", "schedule", "daily.during-reset"]))
        .rejects.toThrow("local test control or reset");
      expect(await exists(started)).toBe(false);
    } finally {
      releaseReset();
      await resetting;
    }
    expect(await exists(path.join(root, ".konae/control.lock"))).toBe(false);
    expect(await exists(path.join(root, ".konae/lifecycle.lock"))).toBe(false);

    await expect(kernel.run(["test", "reply", "fail"], "{}")).rejects.toThrow("controlled operator failure");
    expect(await exists(path.join(root, ".konae/control.lock"))).toBe(false);
    await writeFile(path.join(root, "data/error-state"), "restore after reset failure");
    await expect(resetData(root, { data: kernel.data, async run() { throw Error("reset operator failure"); } }, undefined))
      .rejects.toThrow("reset operator failure");
    expect(await readFile(path.join(root, "data/error-state"), "utf8")).toBe("restore after reset failure");
    expect(await exists(path.join(root, ".konae/control.lock"))).toBe(false);
    expect(await exists(path.join(root, ".konae/lifecycle.lock"))).toBe(false);
    expect(heldResetCalls).toEqual(["test reset-effects"]);

    await rm(started, { force: true });
    await rm(gate, { force: true });
    const replyFile = path.join(root, "reply.json");
    await writeFile(replyFile, JSON.stringify({ responseNumber: 1, error: "controlled" }));
    const cli = fileURLToPath(new URL("./konae.ts", import.meta.url));
    const signalChild = Bun.spawn([process.execPath, "run", cli, "test", "reply", "call-signal", "--file", replyFile, "--dir", root], {
      env: { ...process.env, KONAE_KERNEL_BIN: binary }, stdout: "pipe", stderr: "pipe",
    });
    try {
      await waitFor(started, signalChild);
      expect(await exists(path.join(root, ".konae/control.lock"))).toBe(true);
      signalChild.kill("SIGINT");
      const [code, output, error] = await Promise.all([signalChild.exited, new Response(signalChild.stdout).text(), new Response(signalChild.stderr).text()]);
      expect({ code, output, error }).toMatchObject({ code: 130 });
      expect(await exists(path.join(root, ".konae/control.lock"))).toBe(false);
    } finally {
      await writeFile(gate, "go");
      if (signalChild.exitCode === null) { signalChild.kill("SIGTERM"); await signalChild.exited; }
    }
  } finally {
    if (previousBinary === undefined) delete process.env.KONAE_KERNEL_BIN;
    else process.env.KONAE_KERNEL_BIN = previousBinary;
  }
});
