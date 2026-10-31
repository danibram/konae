// Probe: what the platform has no primitive for. Decides the matrix cells for
// email on an event, realtime push and third-party login, by reading what the
// runtime door offers and how the two doors answer a declaration for each.
// Scheduled work and the custom domain have primitives since 2026-11-07; the
// schedule and domain probes prove them, and this one checks that both doors
// refuse the forms they do not take and what an unclaimed hostname answers.
//
//   KONAE_PROBE_PORT=23050 bun run examples/probes/platform/replay.ts
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import type { McpClient } from "../../../src/mcp/client";
import { probe } from "../_kit";

const here = import.meta.dir;
const toolNames = async (client: McpClient) => {
  await client.connect();
  const listed = await (client as any).request("tools/list", {});
  return (listed.tools as { name: string }[]).map((tool) => tool.name).sort();
};

await probe("platform", 23050, async (kit) => {
  const { url } = kit;
  const app = await kit.publish(here, { projectId: "probe_platform", visibility: "public", signup: { mode: "open" } });
  const cookie = await kit.signup(app.host, "nell@probe.example");

  // Every tool the runtime door offers. None sends mail, schedules, subscribes, charges or signs in elsewhere:
  // a schedule is a declaration the Kernel's timer runs, never a tool an app calls.
  const member = await toolNames(kit.runtime(app.host, cookie));
  const visitor = await toolNames(kit.runtime(app.host));
  const absent = /mail|schedule|cron|subscribe|stream|watch|pay|charge|oauth|provider|domain/;
  for (const name of [...member, ...visitor]) assert(!absent.test(name), `${name} looks like a primitive this probe says is missing.`);
  assert(member.includes("actions_invoke") && member.includes("files_upload") && visitor.includes("auth_signup"));
  kit.pass("Runtime tools", `A signed-in app user gets ${member.join(", ")}. A visitor gets ${visitor.join(", ")}. Accounts are email and password only.`);

  // A declaration for each missing primitive, at both doors.
  const schema = await readFile(path.join(here, "schema.sql"), "utf8");
  const program = await readFile(path.join(here, "actions.json"), "utf8");
  const variants: Record<string, (actions: any) => void> = {
    "an action of kind email": (actions) => { actions["notes.mail"] = { kind: "email", access: "authenticated", statement: "", bindings: [] }; },
    "a schedule on a mutation": (actions) => { actions["notes.add"].schedule = "0 8 * * *"; },
    "a subscription on a query": (actions) => { actions["notes.due"].subscribe = true; },
  };
  // Today's answers: one refusal with the same words at both doors for each. Until 2026-11-07 the
  // schedule and the subscription passed both doors and did nothing. notes.add takes input, so a
  // schedule on it is refused before its cron text is read; the schedule probe covers the rest.
  const same = (message: string): [string, string] => [message, message];
  const expected: Record<string, [string, string]> = {
    "an action of kind email": same('Action notes.mail has an invalid kind. Use "query", "mutation", "signal" or "call".'),
    "a schedule on a mutation": same("Action notes.add declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values."),
    "a subscription on a query": same("Action notes.due declares subscribe, but the Kernel pushes no updates to a page. Poll a query that reads rows after a cursor instead."),
  };
  for (const [label, change] of Object.entries(variants)) {
    const variant = JSON.parse(program);
    change(variant.actions);
    const files = { "schema.sql": schema, "actions.json": JSON.stringify(variant) };
    const gate = await kit.gateRefusal(files).then((answer) => answer.detail ?? answer.message, (error: Error) => `passes (${error.message})`);
    const kernel = await kit.planRefusal(here, `probe_platform_${label.replaceAll(" ", "_")}`, files).catch(() => "passes");
    assert.deepEqual([gate.startsWith("passes") ? "passes" : gate, kernel], expected[label], label);
    console.log(`  ${label}: Gate ${gate.startsWith("passes") ? "passes it" : `"${gate}"`}; Kernel ${kernel === "passes" ? "publishes it" : `"${kernel}"`}`);
    kit.pass(`Declare ${label}`, `Gate: ${gate.startsWith("passes") ? "passes it" : `"${gate}"`}. Kernel: ${kernel === "passes" ? "publishes it and runs nothing extra" : `"${kernel}"`}.`);
  }

  // Delayed work without a worker: a reminder is a row a query reads once its time has come.
  const runtime = kit.runtime(app.host, cookie);
  await runtime.call("actions_invoke", { name: "notes.add", input: { body: "Water the plants", remind_at: Date.now() + 1200 } });
  assert.equal((await runtime.call<any[]>("actions_invoke", { name: "notes.due", input: {} })).length, 0);
  await Bun.sleep(1400);
  assert.equal((await runtime.call<any[]>("actions_invoke", { name: "notes.due", input: {} })).length, 1);
  kit.pass("A reminder without a worker", "notes.due returns nothing before remind_at and the note after it, because the query compares remind_at with kernel.now when someone asks. To act when nobody asks, a mutation declares a schedule (schedule probe); sending the reminder by email still has no primitive.");

  // A hostname no Deployment has claimed with domain_set answers like an unknown host.
  const custom = await fetch(`${url}/`, { headers: { host: "notes.example.org" } });
  const customBody = (await custom.text()).replace(/\s+/g, " ").slice(0, 120);
  assert.notEqual(custom.status, 200);
  console.log(`  Host: notes.example.org answers ${custom.status} ${customBody}`);
  // A proxy that maps the domain and rewrites Host still carries the browser's Origin, which the runtime refuses.
  const proxied = await fetch(`${url}/_konae/mcp`, {
    method: "POST",
    headers: { host: app.host, origin: "https://notes.example.org", "content-type": "application/json", accept: "application/json, text/event-stream", "x-forwarded-for": kit.address() },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "probe", version: "1" } } }),
  });
  const proxiedBody = (await proxied.text()).slice(0, 160);
  assert.notEqual(proxied.status, 200);
  kit.pass("Unclaimed hostname", `A request for Host: notes.example.org, which no domain_set claimed, answers ${custom.status}. Behind a proxy that rewrites Host to ${app.host}, the page's runtime call carries Origin: https://notes.example.org and answers ${proxied.status} ${proxiedBody}.`);
});
