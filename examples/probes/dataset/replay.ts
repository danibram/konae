// Probe: a public dataset with a JSON read API and a private write API.
// Proves search, pagination and sorting, counters and
// aggregates, the public read API from another origin, and the keyed write API.
//
//   KONAE_PROBE_PORT=23000 bun run examples/probes/dataset/replay.ts
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { probe } from "../_kit";

const here = import.meta.dir;

await probe("dataset", 23000, async (kit) => {
  const { url, owner } = kit;
  const app = await kit.publish(here, { projectId: "probe_dataset", visibility: "public", signup: { mode: "closed" } });
  const regions = ["north", "south", "delta"];
  const rows = Array.from({ length: 30 }, (_, index) => {
    const id = index + 1;
    const name = id % 7 === 0 ? `Bridge ${id}` : `Station ${id}`;
    return `(${id}, '${name}', '${regions[index % 3]}', ${(id * 0.37).toFixed(2)})`;
  });
  await owner.call("database_execute", { deploymentId: app.deploymentId, operation: "sql", input: { sql: `INSERT INTO stations (id, name, region, level) VALUES ${rows.join(", ")}` } });
  const get = async (route: string, origin?: string) => {
    const answer = await kit.appFetch(app.host)(url + route, origin ? { headers: { origin } } : {});
    return { status: answer.status, headers: answer.headers, body: await answer.json() as any };
  };

  // Search: LIKE over a column, combined with an optional filter.
  const bridges = await get("/api/stations?q=Bridge");
  assert.equal(bridges.status, 200);
  assert.deepEqual(bridges.body.result.map((row: any) => row.id), [7, 14, 21, 28]);
  const northBridges = await get("/api/stations?q=Bridge&region=north");
  assert.deepEqual(northBridges.body.result.map((row: any) => row.id), [7, 28]);
  kit.pass("Search", "GET /api/stations?q=Bridge returns ids 7, 14, 21, 28 through name LIKE '%' || ?1 || '%'; adding region=north narrows to 7, 28 with a nullable binding.");

  // Full-text search needs a virtual table, which schema.sql may not create.
  const ftsSchema = (await readFile(path.join(here, "schema.sql"), "utf8")) + "\nCREATE VIRTUAL TABLE IF NOT EXISTS stations_fts USING fts5(name);\n";
  const actionsJson = await readFile(path.join(here, "actions.json"), "utf8");
  const ftsKernel = await kit.planRefusal(here, "probe_dataset_fts", { "schema.sql": ftsSchema });
  const ftsGate = await kit.gateRefusal({ "schema.sql": ftsSchema, "actions.json": actionsJson });
  assert.equal(ftsKernel, "schema.sql may only create tables and indexes, but it contains CREATE VIRTUAL.");
  assert.equal(ftsGate.message, ftsKernel);
  kit.pass("Full-text search refused", `Both doors refuse an FTS5 table: "${ftsKernel}"`);

  // Offset pagination with a caller-chosen page size and sort.
  const page1 = await get("/api/stations?sort=level&limit=5");
  const page2 = await get("/api/stations?sort=level&limit=5&offset=5");
  assert.deepEqual(page1.body.result.map((row: any) => row.id), [30, 29, 28, 27, 26]);
  assert.deepEqual(page2.body.result.map((row: any) => row.id), [25, 24, 23, 22, 21]);
  const byName = await get("/api/stations?sort=name&limit=3");
  assert.deepEqual(byName.body.result.map((row: any) => row.name), ["Bridge 14", "Bridge 21", "Bridge 28"]);
  const capped = await get("/api/stations?limit=5000");
  assert.equal(capped.body.result.length, 30);
  // A binding is a value, never a column name, so an unknown sort falls back to id.
  const unknownSort = await get("/api/stations?sort=stars&limit=3");
  assert.deepEqual(unknownSort.body.result.map((row: any) => row.id), [1, 2, 3]);
  const injected = await get(`/api/stations?sort=${encodeURIComponent("level; DROP TABLE stations")}&limit=3`);
  assert.equal(injected.status, 400);
  console.log(`  sort=level; DROP TABLE stations answers ${injected.status} ${JSON.stringify(injected.body)}`);
  kit.pass("Pagination and sorting", "LIMIT min(coalesce(?4, 20), 100) OFFSET coalesce(?5, 0) pages 30, 29, 28, 27, 26 then 25 to 21 by level; sort=name orders by name; the sort key chooses among CASE branches the statement names, and any other value orders by id.");

  // Keyset pagination: the cursor a program follows.
  const after = await get("/api/stations/after?after=10");
  assert.deepEqual(after.body.result.map((row: any) => row.id), [11, 12, 13, 14, 15, 16, 17, 18, 19, 20]);
  kit.pass("Keyset pagination", "GET /api/stations/after?after=10 returns ids 11 to 20.");

  // Aggregates: one row per group, totals that add up.
  const summary = await get("/api/summary");
  assert.deepEqual(summary.body.result.map((row: any) => [row.region, row.stations]), [["delta", 10], ["north", 10], ["south", 10]]);
  assert.equal(summary.body.result.find((row: any) => row.region === "delta").peak, 11.1);
  kit.pass("Aggregates", "GET /api/summary groups 30 stations into 3 regions of 10 with count, avg and max.");

  // The read API from a page on another origin. A GET route that answers anyone
  // allows every origin since ADR-0013; a page lists the origin in its connects.
  const foreign = await get("/api/summary", "https://dashboard.example");
  assert.equal(foreign.status, 200);
  assert.equal(foreign.headers.get("access-control-allow-origin"), "*");
  assert.equal(foreign.headers.get("cache-control"), "no-store");
  const preflight = await kit.appFetch(app.host)(`${url}/api/summary`, { method: "OPTIONS", headers: { origin: "https://dashboard.example", "access-control-request-method": "GET" } });
  assert.equal(preflight.headers.get("access-control-allow-origin"), null);
  kit.pass("Read API from another origin", `A GET with Origin: https://dashboard.example answers 200 with Access-Control-Allow-Origin: * and Cache-Control: no-store, so a server reads it and so does a page on another origin whose policy allows this host. An OPTIONS preflight answers ${preflight.status} with no CORS headers, so only a simple GET crosses origins.`);

  // The write API: a key that names the action, nothing else.
  const write = (body: object, key?: string) => kit.appFetch(app.host)(`${url}/api/readings`, {
    method: "POST", headers: { "content-type": "application/json", ...(key ? { authorization: `Bearer ${key}` } : {}) }, body: JSON.stringify(body),
  });
  const anonymous = await write({ station: 3, level: 9.5 });
  assert.equal(anonymous.status, 401);
  assert.deepEqual(await anonymous.json(), { error: "authentication-required" });
  const issued = await owner.call<{ key: string }>("api_keys_create", { deploymentId: app.deploymentId, name: "gauge uploader", actions: ["readings.record"], files: [] });
  const written = await write({ station: 3, level: 9.5 }, issued.key);
  assert.equal(written.status, 200);
  assert.equal((await written.json()).result.changes, 1);
  assert.equal((await get("/api/stations/after?after=2")).body.result[0].level, 9.5);
  const unknown = await write({ station: 999, level: 1 }, issued.key);
  assert.equal(unknown.status, 403);
  assert.deepEqual(await unknown.json(), { error: "No station has that id." });
  kit.pass("Private write API", "POST /api/readings with \"callers\": [\"key\"] answers 401 authentication-required to a visitor, 200 { changes: 1 } to a key that names readings.record, and 403 with the guard for an unknown station.");
});
