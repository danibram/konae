import { readFile } from "node:fs/promises";

export type LocalTestRunner = { run(args: string[], input?: string): Promise<string> };
export type TestScenario = {
  version: 1;
  responses: Array<{
    action: string;
    match: Record<string, unknown>;
    response: { responseNumber: number; status?: number; body?: unknown; error?: string };
  }>;
};

function parse<T>(text: string): T {
  try { return JSON.parse(text) as T; }
  catch { throw Error("Kernel returned invalid JSON."); }
}

function isSubset(actual: unknown, expected: unknown): boolean {
  if (expected === null || typeof expected !== "object" || Array.isArray(expected)) return JSON.stringify(actual) === JSON.stringify(expected);
  if (!actual || typeof actual !== "object" || Array.isArray(actual)) return false;
  return Object.entries(expected as Record<string, unknown>).every(([key, value]) => isSubset((actual as Record<string, unknown>)[key], value));
}

export async function testStatus(kernel: LocalTestRunner) {
  return parse(await kernel.run(["test", "status"]));
}

export async function testCalls(kernel: LocalTestRunner) {
  return parse<{ calls: Array<Record<string, unknown> & { callId: string; action: string; responseNumber: number }> }>(await kernel.run(["test", "calls"]));
}

export async function testReply(kernel: LocalTestRunner, callId: string, body: string) {
  const value = parse<Record<string, unknown>>(body);
  if (!Number.isInteger(value.responseNumber) || Number(value.responseNumber) < 1) throw Error("Reply JSON needs a positive responseNumber.");
  const hasStatus = Number.isInteger(value.status) && Number(value.status) >= 100 && Number(value.status) <= 599 && Object.hasOwn(value, "body");
  const hasError = typeof value.error === "string" && value.error.length > 0;
  if (hasStatus === hasError) throw Error("Reply JSON needs either status (100 to 599) with body, or a non-empty error.");
  return parse(await kernel.run(["test", "reply", callId], JSON.stringify(value)));
}

export async function testSchedule(kernel: LocalTestRunner, action: string) {
  return parse(await kernel.run(["test", "schedule", action]));
}

export async function runScenario(kernel: LocalTestRunner, scenarioFile: string) {
  let scenario: TestScenario;
  try { scenario = JSON.parse(await readFile(scenarioFile, "utf8")) as TestScenario; }
  catch (error) { throw Error(`Could not read scenario JSON: ${error instanceof Error ? error.message : String(error)}`); }
  if (scenario?.version !== 1 || !Array.isArray(scenario.responses) || !scenario.responses.length) throw Error("Scenario needs version 1 and a non-empty responses list.");
  const { calls } = await testCalls(kernel);
  const awaiting = calls.filter((call) => typeof call.awaitingAt === "number");
  const results = [];
  for (const step of scenario.responses) {
    if (!step || typeof step.action !== "string" || !step.match || typeof step.match !== "object" || !step.response || !Number.isInteger(step.response.responseNumber)) throw Error("Each scenario response needs action, match, and response.responseNumber.");
    const candidates = awaiting.filter((call) => call.action === step.action && isSubset(call, step.match));
    if (candidates.length !== 1) throw Error(`Scenario step for ${step.action} matched ${candidates.length} awaiting calls. Refusing to guess.`);
    const call = candidates[0];
    if (call.responseNumber !== step.response.responseNumber) throw Error(`Scenario step for ${step.action} expected response ${call.responseNumber}, got ${step.response.responseNumber}.`);
    const result = await testReply(kernel, call.callId, JSON.stringify(step.response));
    results.push({ callId: call.callId, action: call.action, responseNumber: call.responseNumber, result });
    awaiting.splice(awaiting.indexOf(call), 1);
  }
  return { applied: results, pending: awaiting.map(({ callId, action, responseNumber, awaitingAt }) => ({ callId, action, responseNumber, awaitingAt })) };
}

export function testControls(kernel: LocalTestRunner) {
  return {
    status: () => testStatus(kernel),
    calls: () => testCalls(kernel),
    reply: (callId: string, body: string) => testReply(kernel, callId, body),
    schedule: (action: string) => testSchedule(kernel, action),
  };
}
