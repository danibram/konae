import { expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { initLocal } from "./konae";

const kernelBinary = process.env.KONAE_KERNEL_BIN;
const integrationTest = kernelBinary ? test : test.skip;

async function freePort() {
  const probe = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response() });
  const port = probe.port;
  probe.stop(true);
  return port;
}

function cli(root: string, args: string[], input?: string) {
  return Bun.spawn([process.execPath, path.resolve("scripts/konae.ts"), ...args, "--dir", root], {
    env: { ...process.env, KONAE_KERNEL_BIN: kernelBinary! },
    stdin: input === undefined ? "ignore" : new Blob([input]), stdout: "pipe", stderr: "pipe",
  });
}

async function resultOf(child: ReturnType<typeof cli>) {
  const [exitCode, stdout, stderr] = await Promise.all([
    child.exited,
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
  ]);
  return { exitCode, stdout, stderr };
}

function countIs(value: unknown, expected: number): boolean {
  if (Array.isArray(value)) return value.some((entry) => countIs(entry, expected));
  if (!value || typeof value !== "object") return false;
  const row = value as Record<string, unknown>;
  return row.count === expected || Object.values(row).some((entry) => countIs(entry, expected));
}

integrationTest("real local test controls preserve retry state and run one paused schedule", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-local-test-integration-"));
  let dev: ReturnType<typeof Bun.spawn> | undefined;
  let outputReaders: Promise<void>[] = [];
  let output = "";
  const record = (stream: ReadableStream<Uint8Array>) => (async () => {
    const decoder = new TextDecoder();
    for await (const chunk of stream) output += decoder.decode(chunk, { stream: true });
  })();

  try {
    const settings = await initLocal(root, "Local test integration");
    settings.visibility = "public";
    settings.signupPolicy = { mode: "open", allowlist: [] };
    await writeFile(path.join(root, "konae.json"), `${JSON.stringify(settings, null, 2)}\n`);
    await writeFile(path.join(root, "app/schema.sql"), "CREATE TABLE IF NOT EXISTS schedule_runs (run_id TEXT NOT NULL);\n");
    await writeFile(path.join(root, "app/actions.json"), `${JSON.stringify({
      version: 1,
      actions: {
        "probe.send": {
          kind: "call", access: "anonymous", method: "POST", url: "https://example.test/hook",
          bindings: [{ source: "input", key: "caseId", type: "string", maxLength: 80 }],
          body: { caseId: "caseId" },
        },
        "probe.tick": {
          kind: "mutation", access: "anonymous", schedule: { every: "1h" },
          statement: "INSERT INTO schedule_runs (run_id) VALUES ('manual')", bindings: [],
        },
      },
    }, null, 2)}\n`);
    await mkdir(path.join(root, "tests"), { recursive: true });

    const port = await freePort();
    let eventsPort = await freePort();
    while (eventsPort === port) eventsPort = await freePort();
    dev = Bun.spawn([process.execPath, path.resolve("scripts/konae.ts"), "dev", "--dir", root, "--port", String(port), "--events-port", String(eventsPort)], {
      env: { ...process.env, KONAE_KERNEL_BIN: kernelBinary! }, stdout: "pipe", stderr: "pipe",
    });
    outputReaders = [record(dev.stdout), record(dev.stderr)];
    let appUrl: string | undefined;
    for (let attempt = 0; attempt < 300; attempt++) {
      if (dev.exitCode !== null) throw Error(`Local dev stopped during startup. ${output}`);
      appUrl = output.match(/http:\/\/[^\s]+/)?.[0];
      if (appUrl) break;
      await Bun.sleep(100);
    }
    expect(appUrl, `Local dev did not start. ${output}`).toBeDefined();
    const appOrigin = new URL(appUrl!);
    const appHost = `${settings.slug}.localhost:${port}`;
    let rpcId = 0;
    const invoke = async (caseId: string) => {
      const response = await fetch(`${appOrigin.origin}/_konae/mcp`, {
        method: "POST", headers: { host: appHost, "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: ++rpcId, method: "tools/call", params: { name: "actions_invoke", arguments: { name: "probe.send", input: { caseId } } } }),
      });
      expect(response.status).toBe(200);
      const envelope = await response.json() as { result?: { isError?: boolean; structuredContent?: { result?: { callId?: string; queued?: boolean } }; content?: Array<{ text?: string }> } };
      expect(envelope.result?.isError).not.toBe(true);
      const queued = envelope.result?.structuredContent?.result ?? JSON.parse(envelope.result?.content?.[0]?.text ?? "null");
      expect(queued).toMatchObject({ queued: true });
      return queued.callId as string;
    };
    const [firstCallId, secondCallId] = await Promise.all([invoke("case-a"), invoke("case-b")]);

    const statusProcess = cli(root, ["test", "status"]);
    const statusResult = await resultOf(statusProcess);
    expect(statusResult.exitCode, statusResult.stderr).toBe(0);
    const status = JSON.parse(statusResult.stdout) as { mode: string; pausedSchedules: Array<{ action: string; paused: boolean }> };
    expect(status.mode).toBe("test");
    expect(status.pausedSchedules).toContainEqual(expect.objectContaining({ action: "probe.tick", paused: true }));

    const initialProcess = cli(root, ["test", "calls"]);
    const initialResult = await resultOf(initialProcess);
    expect(initialResult.exitCode, initialResult.stderr).toBe(0);
    const initial = JSON.parse(initialResult.stdout) as { calls: Array<Record<string, unknown>> };
    expect(initial.calls).toEqual(expect.arrayContaining([
      expect.objectContaining({ callId: firstCallId, action: "probe.send", input: { caseId: "case-a" }, deployment: settings.id, awaitingAt: expect.any(Number) }),
      expect.objectContaining({ callId: secondCallId, action: "probe.send", input: { caseId: "case-b" }, deployment: settings.id, awaitingAt: expect.any(Number) }),
    ]));

    const scenarioFile = path.join(root, "tests/retry-503.json");
    await writeFile(scenarioFile, `${JSON.stringify({ version: 1, responses: [{
      action: "probe.send", match: { input: { caseId: "case-a" } },
      response: { responseNumber: 1, status: 503, body: { retry: true } },
    }] })}\n`);
    const scenarioProcess = cli(root, ["test", "scenario", "tests/retry-503.json"]);
    const scenarioResult = await resultOf(scenarioProcess);
    expect(scenarioResult.exitCode, scenarioResult.stderr).toBe(0);
    const scenario = JSON.parse(scenarioResult.stdout) as { applied: Array<Record<string, unknown>>; pending: Array<Record<string, unknown>> };
    expect(scenario.applied).toEqual([expect.objectContaining({ callId: firstCallId, action: "probe.send", responseNumber: 1, result: { accepted: true, callId: firstCallId, responseNumber: 1 } })]);
    const retried = scenario.pending.find((call) => call.callId === firstCallId)!;
    expect(retried).toMatchObject({ state: "pending", responseNumber: 2, attempts: 1, awaitingAt: null });
    expect(retried.nextAttemptAt).toEqual(expect.any(Number));
    expect(Number(retried.nextAttemptAt)).toBeGreaterThan(Date.now());
    expect(scenario.pending).toContainEqual(expect.objectContaining({ callId: secondCallId, input: { caseId: "case-b" } }));

    const duplicateProcess = cli(root, ["test", "reply", firstCallId, "--stdin"], JSON.stringify({ responseNumber: 1, status: 503, body: { retry: true } }));
    const duplicateResult = await resultOf(duplicateProcess);
    expect(duplicateResult.exitCode).not.toBe(0);
    expect(JSON.parse(duplicateResult.stdout)).toMatchObject({ accepted: false, callId: firstCallId, responseNumber: 1 });
    const afterDuplicateProcess = cli(root, ["test", "calls"]);
    const afterDuplicateResult = await resultOf(afterDuplicateProcess);
    expect(afterDuplicateResult.exitCode, afterDuplicateResult.stderr).toBe(0);
    const afterDuplicate = JSON.parse(afterDuplicateResult.stdout) as { calls: Array<Record<string, unknown>> };
    expect(afterDuplicate.calls.find((call) => call.callId === firstCallId)).toMatchObject({ responseNumber: 2, attempts: 1 });

    const scheduleProcess = cli(root, ["test", "schedule", "probe.tick"]);
    const scheduleResult = await resultOf(scheduleProcess);
    expect(scheduleResult.exitCode, scheduleResult.stderr).toBe(0);
    expect(JSON.parse(scheduleResult.stdout)).toMatchObject({ changes: 1 });
    const sqlProcess = cli(root, ["sql", "SELECT COUNT(*) AS count FROM schedule_runs"]);
    const sqlResult = await resultOf(sqlProcess);
    expect(sqlResult.exitCode, sqlResult.stderr).toBe(0);
    expect(countIs(JSON.parse(sqlResult.stdout), 1)).toBe(true);
  } finally {
    if (dev && dev.exitCode === null) {
      dev.kill("SIGTERM");
      await Promise.race([dev.exited, Bun.sleep(5000)]);
      if (dev.exitCode === null) dev.kill("SIGKILL");
      await dev.exited;
    }
    if (outputReaders.length) await Promise.all(outputReaders);
    await rm(root, { recursive: true, force: true });
  }
}, 90_000);
