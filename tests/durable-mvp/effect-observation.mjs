import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { testInstallation } from "./harness.mjs";
import { release } from "../kernel-harness.ts";
import { McpClient } from "../../src/mcp/client.ts";
import { Database } from "bun:sqlite";

const repo = process.argv[2] ?? process.cwd();
const root = await mkdtemp(join(tmpdir(), "konae-effect-observation-"));
const fixture = await testInstallation(join(repo, "kernel-rs/target/debug/konae"), { root, port: 4249, env: { TOKIO_WORKER_THREADS: "6" } });
const projectId = "effect-observation-private-fixture";
const conversationId = crypto.randomUUID();
const standbyId = crypto.randomUUID();
const device = "effect-observation-fixture-device";
let rpcId = 0;
try {
  const owner = await fixture.owner("effect-observation-fixture-owner");
  const client = (session) => new McpClient(fixture.origin + "/mcp", {
    fetch: (input, options) => {
      const headers = new Headers(options?.headers);
      headers.set("cookie", session.cookie);
      return fetch(input, { ...options, headers });
    },
  });
  const mcp = client(owner);
  await mcp.call("project_create", { projectId, name: "Effect observation fixture" });
  await release(mcp, {
    project: { id: projectId, name: "Effect observation fixture", slug: projectId, manifest: { version: 1, visibility: "private" }, files: {
      "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL);",
      "actions.json": JSON.stringify({ version: 1, actions: {} }),
    } },
    visibility: "private",
    artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Private fixture</main>" } },
    initialModules: { auth: { on: true }, actions: { on: true } },
  }, projectId);
  await mcp.call("database_execute", { projectId, operation: "sql", input: { sql: "INSERT INTO notes(title) VALUES ('before')" } });

  const base = `/_konae/projects/${projectId}/conversations`;
  const conversation = `${base}/${conversationId}`;
  const request = (path, session, method = "GET", body) => fixture.request(path, { cookie: session.cookie, method, body, headers: { connection: "close" } });
  const headersFor = ({ id, observes, conversation = conversationId, executor = device, generation = 1 } = {}) => ({
    "x-konae-execution-project": projectId,
    "x-konae-execution-conversation": conversation,
    "x-konae-execution-id": executor,
    "x-konae-execution-generation": String(generation),
    "x-konae-execution-call": id ?? crypto.randomUUID(),
    ...(observes ? { "x-konae-execution-observes": observes } : {}),
  });
  const callAgent = (name, args, extra = {}) => fixture.request("/mcp", {
    method: "POST", cookie: owner.cookie, headers: { ...headersFor(extra), accept: "application/json, text/event-stream", connection: "close" },
    body: { jsonrpc: "2.0", id: ++rpcId, method: "tools/call", params: { name, arguments: args } },
  });
  const succeeded = (response) => response.status === 200 && response.json?.result && !response.json.result.isError;
  for (const id of [conversationId, standbyId]) {
    const created = await request(base, owner, "POST", { id, formatVersion: 1, piVersion: "1.5.2", registryVersion: "1" });
    assert.ok([200, 201].includes(created.status), created.text);
  }
  const claimed = await request(`${conversation}/claim`, owner, "POST", { expectedGeneration: 0, executorId: device, continueExplicitly: true });
  assert.equal(claimed.status, 200, claimed.text);
  assert.equal(claimed.json.generation, 1);
  assert.equal((await mcp.call("session_inspect")).can.read, true);

  const email = "synthetic-lost-ack@example.test";
  const inviteId = "fixture-invitation-lost-ack";
  const invite = await callAgent("app_invitations_create", { projectId, emails: [email], idempotencyKey: "lost-ack-fixture" }, { id: inviteId });
  assert.ok(succeeded(invite), invite.text);
  const database = new Database(join(root, "mcp.sqlite"), { strict: true });
  database.query("UPDATE conversation_effects SET state='running' WHERE owner=(SELECT owner FROM conversation_effects WHERE call_id=?) AND project=? AND conversation=? AND call_id=?").run(inviteId, projectId, conversationId, inviteId);
  assert.equal((await request(`${conversation}/effects/${inviteId}`, owner)).json.state, "unknown", "read-only receipt GET projects an inactive running row as unknown");
  assert.equal(database.query("SELECT state FROM conversation_effects WHERE call_id=?").get(inviteId).state, "running", "receipt GET does not write the database");
  const deniedDelete = await request(conversation, owner, "DELETE");
  assert.equal(deniedDelete.status, 409, deniedDelete.text);
  assert.match(deniedDelete.text, /running or unresolved external effect/i);

  const invitationBefore = await mcp.call("app_invitations_list", { projectId });
  assert.equal(invitationBefore.invitations.filter((item) => item.email === email).length, 1);
  const unlinkedRead = await mcp.call("app_invitations_list", { projectId });
  assert.equal((await request(`${conversation}/effects/${inviteId}`, owner)).json.state, "unknown", "the first unlinked read leaves the fence pending");
  const invitationReadback = await callAgent("app_invitations_list", { projectId }, { id: "fixture-invitation-readback", observes: inviteId });
  assert.ok(succeeded(invitationReadback), invitationReadback.text);
  assert.equal((await request(`${conversation}/effects/${inviteId}`, owner)).json.state, "observed");

  const sqlWriteId = "fixture-sql-write";
  const write = await callAgent("database_execute", { projectId, operation: "sql", input: { sql: "UPDATE notes SET title = 'after' WHERE id = 1" } }, { id: sqlWriteId });
  assert.ok(succeeded(write), write.text);
  database.query("UPDATE conversation_effects SET state='running' WHERE owner=(SELECT owner FROM conversation_effects WHERE call_id=?) AND project=? AND conversation=? AND call_id=?").run(sqlWriteId, projectId, conversationId, sqlWriteId);
  database.query("UPDATE conversation_execution_leases SET expires_at=0 WHERE project=? AND conversation=?").run(projectId, conversationId);
  assert.equal((await request(`${conversation}/effects/${sqlWriteId}`, owner)).json.state, "unknown");
  const sqlDiagnostic = await mcp.call("database_execute", { projectId, operation: "sql", input: { sql: "SELECT id,title FROM notes WHERE id = 999 LIMIT 10;" } });
  assert.equal(sqlDiagnostic.rows.length, 0, "an empty diagnostic read is available as evidence before confirmation");
  assert.equal((await request(`${conversation}/effects/${sqlWriteId}`, owner)).json.state, "unknown", "expired-lease diagnostic read does not resolve the pending write");
  const sqlRead = await callAgent("database_execute", { projectId, operation: "sql", input: { sql: " SELECT id,title FROM notes WHERE id = 1 LIMIT 10; " } }, { id: "fixture-sql-readback", observes: sqlWriteId });
  assert.ok(succeeded(sqlRead), sqlRead.text);
  assert.equal((await request(`${conversation}/effects/${sqlWriteId}`, owner)).json.state, "observed");
  assert.match(JSON.stringify(sqlRead.json), /after/);
  const deleted = await request(conversation, owner, "DELETE");
  assert.equal(deleted.status, 200, deleted.text);
  const newClaim = await request(`${base}/${standbyId}/claim`, owner, "POST", { expectedGeneration: 1, executorId: "fixture-observer", continueExplicitly: true });
  assert.equal(newClaim.status, 200, newClaim.text);
  const finalWriteId = "fixture-project-epoch-write";
  const finalWrite = await callAgent("database_execute", { projectId, operation: "sql", input: { sql: "UPDATE notes SET title = 'for deleted project' WHERE id = 1" } }, { id: finalWriteId, conversation: standbyId, executor: "fixture-observer", generation: newClaim.json.generation });
  assert.ok(succeeded(finalWrite), finalWrite.text);
  assert.equal((await request(`${base}/${standbyId}/effects/${finalWriteId}`, owner)).json.state, "unknown");
  assert.equal((await mcp.call("project_delete", { projectId, name: "Effect observation fixture" })).deleted, true);
  assert.ok(!(await mcp.call("projects_list")).projects.some(({ project }) => project.id === projectId));
  assert.equal(database.query("SELECT COUNT(*) AS count FROM conversation_effects WHERE project=?").get(projectId).count, 0);
  await mcp.call("project_create", { projectId, name: "Recreated effect observation fixture" });
  assert.equal((await request(`${base}/${standbyId}`, owner)).status, 404);
  const freshId = crypto.randomUUID();
  assert.ok([200, 201].includes((await request(base, owner, "POST", { id: freshId, formatVersion: 1, piVersion: "1.5.2", registryVersion: "1" })).status));
  const freshLease = await request(`${base}/${freshId}/lease`, owner);
  assert.equal(freshLease.json.unresolvedEffects, false);
  assert.ok(freshLease.json.generation > newClaim.json.generation);
  assert.equal((await request(`${base}/${freshId}/claim`, owner, "POST", { expectedGeneration: freshLease.json.generation, executorId: "fixture-recreated-project", continueExplicitly: true })).status, 200);
  database.close();
  console.log("Loopback HTTP: unresolved effect blocks conversation deletion; documented invitation and bounded SQL read-backs preserve evidence; conversation deletion follows observation; deleting and recreating the whole Project clears old receipts and fences the old generation.");
} finally {
  await fixture.close();
}
