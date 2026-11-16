import { lstat, open, realpath, stat } from "node:fs/promises";
import path from "node:path";

export type LocalTestRunner = { run(args: string[], input?: string): Promise<string> };
export type TestScenario = {
  version: 1;
  responses: Array<{
    action: string;
    match: Record<string, unknown>;
    response: { responseNumber: number; status?: number; body?: unknown; error?: string };
  }>;
};
export type LocalResetSummary = { executionId: string; snapshotId: string; resetAt: number; callsRetired: number; mailRetired: number };

export function localResetSummary(state: unknown): LocalResetSummary | undefined {
  if (!state || typeof state !== "object" || Array.isArray(state)) return undefined;
  const value = state as Record<string, unknown>;
  if (typeof value.executionId !== "string" || value.executionId.length > 256 || typeof value.snapshotId !== "string" || value.snapshotId.length > 256
    || !Number.isSafeInteger(value.resetAt) || Number(value.resetAt) < 0 || !Number.isSafeInteger(value.callsRetired) || Number(value.callsRetired) < 0
    || !Number.isSafeInteger(value.mailRetired) || Number(value.mailRetired) < 0) return undefined;
  return { executionId: value.executionId, snapshotId: value.snapshotId, resetAt: value.resetAt as number, callsRetired: value.callsRetired as number, mailRetired: value.mailRetired as number };
}

const REPLY_MAX_BYTES = 64 * 1024;
const SCENARIO_MAX_BYTES = 1024 * 1024;

function object(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw Error(`${label} must be a JSON object.`);
  return value as Record<string, unknown>;
}

function exactKeys(value: Record<string, unknown>, allowed: string[], label: string) {
  const unknown = Object.keys(value).filter((key) => !allowed.includes(key));
  if (unknown.length) throw Error(`${label} has unknown field${unknown.length === 1 ? "" : "s"}: ${unknown.join(", ")}.`);
}

async function readBoundedFile(filename: string, limit: number) {
  const info = await stat(filename);
  if (!info.isFile() || info.size > limit) throw Error(`File must be a regular file no larger than ${limit} bytes.`);
  const handle = await open(filename, "r");
  try {
    const chunks: Uint8Array[] = []; let length = 0; let position = 0;
    for (;;) {
      const chunk = new Uint8Array(Math.min(64 * 1024, limit + 1 - length));
      const { bytesRead } = await handle.read(chunk, 0, chunk.length, position);
      if (!bytesRead) break;
      length += bytesRead; position += bytesRead;
      if (length > limit) throw Error(`File must be no larger than ${limit} bytes.`);
      chunks.push(chunk.subarray(0, bytesRead));
    }
    const bytes = new Uint8Array(length); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } finally { await handle.close(); }
}

export async function readReplyInput(stream: ReadableStream<Uint8Array>) {
  const reader = stream.getReader(); const chunks: Uint8Array[] = []; let length = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    length += value.byteLength;
    if (length > REPLY_MAX_BYTES) { await reader.cancel(); throw Error(`Reply JSON exceeds ${REPLY_MAX_BYTES} bytes.`); }
    chunks.push(value);
  }
  const bytes = new Uint8Array(length); let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
}

function parse<T>(text: string, label = "Kernel"): T {
  try { return JSON.parse(text) as T; }
  catch { throw Error(`${label} returned invalid JSON.`); }
}

function isSubset(actual: unknown, expected: unknown): boolean {
  if (expected === null || typeof expected !== "object" || Array.isArray(expected)) return JSON.stringify(actual) === JSON.stringify(expected);
  if (!actual || typeof actual !== "object" || Array.isArray(actual)) return false;
  return Object.entries(expected as Record<string, unknown>).every(([key, value]) => isSubset((actual as Record<string, unknown>)[key], value));
}

export async function testStatus(kernel: LocalTestRunner) {
  const status = object(parse(await kernel.run(["test", "status"])), "Kernel status result");
  if (status.mode !== "test" || !Array.isArray(status.pausedSchedules) || !status.pausedSchedules.every((entry) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) return false;
    const schedule = entry as Record<string, unknown>;
    return typeof schedule.action === "string" && typeof schedule.schedule === "string" && schedule.paused === true;
  })) throw Error("Kernel status result needs test mode and paused schedule records.");
  return status;
}

export async function testCalls(kernel: LocalTestRunner) {
  const answer = object(parse(await kernel.run(["test", "calls"])), "Kernel calls result");
  if (!Array.isArray(answer.calls) || !answer.calls.every((call) => call && typeof call === "object" && !Array.isArray(call)
    && typeof (call as Record<string, unknown>).callId === "string" && typeof (call as Record<string, unknown>).action === "string"
    && Number.isInteger((call as Record<string, unknown>).responseNumber) && Number((call as Record<string, unknown>).responseNumber) > 0)) throw Error("Kernel calls result needs call objects with a callId, action and positive responseNumber.");
  return answer as { calls: Array<Record<string, unknown> & { callId: string; action: string; responseNumber: number }> };
}

export async function testReply(kernel: LocalTestRunner, callId: string, body: string) {
  if (new TextEncoder().encode(body).length > REPLY_MAX_BYTES) throw Error(`Reply JSON exceeds ${REPLY_MAX_BYTES} bytes.`);
  const value = object(parse(body, "Reply JSON"), "Reply JSON");
  if (!Number.isInteger(value.responseNumber) || Number(value.responseNumber) < 1) throw Error("Reply JSON needs a positive responseNumber.");
  if (Object.hasOwn(value, "status")) {
    exactKeys(value, ["responseNumber", "status", "body"], "Reply JSON");
    if (!Number.isInteger(value.status) || Number(value.status) < 100 || Number(value.status) > 599 || !Object.hasOwn(value, "body")) throw Error("Reply JSON needs status from 100 to 599 and a body.");
  } else {
    exactKeys(value, ["responseNumber", "error"], "Reply JSON");
    if (typeof value.error !== "string" || !value.error.length) throw Error("Reply JSON needs either status with body or a non-empty error.");
  }
  const result = object(parse(await kernel.run(["test", "reply", callId], JSON.stringify(value))), "Kernel reply result");
  if (typeof result.accepted !== "boolean") throw Error("Kernel reply result needs an accepted boolean.");
  return result as { accepted: boolean } & Record<string, unknown>;
}

export async function testSchedule(kernel: LocalTestRunner, action: string) {
  return object(parse(await kernel.run(["test", "schedule", action])), "Kernel schedule result");
}

export class ScenarioRefusal extends Error {
  constructor(readonly result: { refused: true; reason: string; applied: unknown[]; pending: unknown[] }) {
    super(result.reason);
    this.name = "ScenarioRefusal";
  }
}

export async function resolveScenarioPath(root: string, requested: string) {
  const testsLexical = path.join(root, "tests");
  if ((await lstat(testsLexical).catch(() => undefined))?.isSymbolicLink()) throw Error("Project tests/ must not be a symbolic link.");
  const testsPath = await realpath(testsLexical).catch(() => { throw Error("Scenario files must live under project tests/."); });
  const requestedPath = await realpath(requested).catch(() => { throw Error("Scenario file does not exist."); });
  if (!requestedPath.startsWith(testsPath + path.sep)) throw Error("Scenario files must resolve inside project tests/.");
  const info = await stat(requestedPath);
  if (!info.isFile()) throw Error("Scenario path must name a regular file.");
  return requestedPath;
}

export async function runScenario(kernel: LocalTestRunner, scenarioFile: string) {
  const applied: Array<Record<string, unknown>> = [];
  let pending: unknown[] = [];
  const refuse = (reason: string): never => { throw new ScenarioRefusal({ refused: true, reason, applied, pending }); };
  let scenario: TestScenario;
  try {
    const parsed = object(parse(await readBoundedFile(scenarioFile, SCENARIO_MAX_BYTES), "Scenario"), "Scenario");
    exactKeys(parsed, ["version", "responses"], "Scenario");
    if (parsed.version !== 1 || !Array.isArray(parsed.responses) || !parsed.responses.length) throw Error("Scenario needs version 1 and a non-empty responses list.");
    scenario = parsed as unknown as TestScenario;
  } catch (error) { refuse(error instanceof Error ? error.message : String(error)); }
  try { pending = (await testCalls(kernel)).calls; }
  catch (error) { refuse(error instanceof Error ? error.message : String(error)); }
  for (const rawStep of scenario.responses) {
    let step: TestScenario["responses"][number];
    try {
      const stepValue = object(rawStep, "Scenario response step");
      exactKeys(stepValue, ["action", "match", "response"], "Scenario response step");
      if (typeof stepValue.action !== "string" || !stepValue.action) throw Error("Scenario response step needs an action.");
      const matchValue = object(stepValue.match, "Scenario match");
      const responseValue = object(stepValue.response, "Scenario response");
      step = { action: stepValue.action, match: matchValue, response: responseValue as TestScenario["responses"][number]["response"] };
      const request = { responseNumber: responseValue.responseNumber, ...Object.fromEntries(Object.entries(responseValue).filter(([key]) => key !== "responseNumber")) };
      // Validate the response shape before matching or changing Kernel state.
      const parsedResponse = object(JSON.parse(JSON.stringify(request)), "Scenario response");
      exactKeys(parsedResponse, ["responseNumber", "status", "body", "error"], "Scenario response");
      if (Object.hasOwn(parsedResponse, "status")) {
        if (Object.hasOwn(parsedResponse, "error") || !Number.isInteger(parsedResponse.status) || Number(parsedResponse.status) < 100 || Number(parsedResponse.status) > 599 || !Object.hasOwn(parsedResponse, "body")) throw Error("Scenario response needs status from 100 to 599 and a body.");
      } else if (typeof parsedResponse.error !== "string" || !parsedResponse.error.length || Object.hasOwn(parsedResponse, "body")) throw Error("Scenario response needs either status with body or a non-empty error.");
      if (!Number.isInteger(responseValue.responseNumber) || Number(responseValue.responseNumber) < 1) throw Error("Scenario response needs a positive responseNumber.");
    } catch (error) { refuse(error instanceof Error ? error.message : String(error)); }
    const awaiting = pending.filter((call) => {
      if (!call || typeof call !== "object") return false;
      const value = call as Record<string, unknown>;
      return Number.isInteger(value.responseNumber) && (value.awaitingAt !== undefined || (value.state === "pending" && value.error === "awaiting-response"));
    });
    const candidates = awaiting.filter((call) => (call as Record<string, unknown>).action === step!.action && isSubset(call, step!.match));
    if (candidates.length !== 1) {
      const retry = pending.filter((call) => call && typeof call === "object" && (call as Record<string, unknown>).action === step!.action)
        .map((call) => (call as Record<string, unknown>).nextAttemptAt)
        .find((at) => typeof at === "number" && at > Date.now());
      if (candidates.length === 0 && typeof retry === "number") refuse(`Scenario retry for ${step!.action} is pending until ${new Date(retry).toISOString()}. No response is due yet.`);
      refuse(`Scenario step for ${step!.action} matched ${candidates.length} awaiting calls. Refusing to guess.`);
    }
    const call = candidates[0] as Record<string, unknown> & { callId: string; action: string; responseNumber: number };
    if (call.responseNumber !== step!.response.responseNumber) refuse(`Scenario step for ${step!.action} expected response ${call.responseNumber}, got ${step!.response.responseNumber}.`);
    let result: { accepted: boolean } & Record<string, unknown>;
    try { result = await testReply(kernel, call.callId, JSON.stringify(step!.response)); }
    catch (error) { refuse(error instanceof Error ? error.message : String(error)); }
    if (!result!.accepted) {
      try { pending = (await testCalls(kernel)).calls; }
      catch (error) { refuse(error instanceof Error ? error.message : String(error)); }
      refuse(`Kernel refused the response for ${call.callId}.`);
    }
    applied.push({ callId: call.callId, action: call.action, responseNumber: call.responseNumber, result: result! });
    try { pending = (await testCalls(kernel)).calls; }
    catch (error) { refuse(error instanceof Error ? error.message : String(error)); }
  }
  return { applied, pending };
}

export async function readReplyFile(filename: string) { return readBoundedFile(filename, REPLY_MAX_BYTES); }

export function testControls(kernel: LocalTestRunner, localState?: () => Promise<unknown>) {
  return {
    status: async () => {
      const status = await testStatus(kernel);
      const reset = await localState?.();
      return reset === undefined ? status : { ...status, localState: reset };
    },
    calls: () => testCalls(kernel),
    reply: (callId: string, body: string) => testReply(kernel, callId, body),
    schedule: (action: string) => testSchedule(kernel, action),
  };
}
