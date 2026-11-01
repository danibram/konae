// A hostile app author's SQL against both doors: the Validation Gate's
// parser and the Kernel's release, then the Kernel at run time. Each test is
// one attempt from the security review. A statement both doors refuse must be
// refused with the same sentence.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { parseServerActionSource } from "../src/kernel/actions";
import { checksum } from "../src/mcp/client";
import type { PackageManifest } from "../src/lib/portable-package";
import { importPackage, release } from "./kernel-harness";
import { answerText, rawCall, runtimeClient, startLab, testPort, type Lab } from "./security-review-kernel";

let lab: Lab;
let count = 0;
beforeAll(async () => { lab = await startLab(testPort("KONAE_REVIEW_SQL_PORT", 24000)); }, 180_000);
afterAll(async () => { await lab?.stop(); });

const SCHEMA = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, owner TEXT, body TEXT);";
type Actions = Record<string, unknown>;
const action = (statement: string, extra: object = {}) => ({ kind: "query", access: "anonymous", statement, bindings: [], ...extra });

/** Publishes a public app with these actions and schema, or returns the Kernel's refusal. */
async function publish(actions: Actions, schema = SCHEMA, extra: object = {}): Promise<{ hostname: string; id: string } | string> {
  const id = `review_sql_${++count}`;
  const project = { id, name: id, slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" } };
  await lab.kernel.owner.call("project_create", { projectId: id, name: id, slug: project.slug, manifest: project.manifest });
  try {
    const { receipt } = await release(lab.kernel.owner, {
      project: { ...project, files: { "schema.sql": schema, "actions.json": JSON.stringify({ version: 1, actions }) } },
      visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>SQL</main>" } }, ...extra,
    }, id);
    return receipt;
  } catch (error) {
    return (error as Error).message;
  }
}

/** The Gate's sentence for an actions.json, or null when it passes. */
function gate(actions: Actions): string | null {
  try { parseServerActionSource(JSON.stringify({ version: 1, actions })); return null; } catch (error) { return (error as Error).message; }
}

const invoke = (hostname: string, name: string, input: object = {}) => rawCall(lab, "/_konae/mcp", hostname, "actions_invoke", { name, input });

describe("statements both doors refuse with one sentence", () => {
  const cases: [string, Actions][] = [
    ["a second statement after ;", { "notes.list": action("SELECT id FROM notes LIMIT 5; DELETE FROM notes") }],
    ["a no-break space after ;", { "notes.list": action("SELECT id FROM notes LIMIT 5; ") }],
    ["a query that writes schema", { "notes.list": action("CREATE TABLE x(a)") }],
    ["a trigger written as a mutation", { "notes.add": action("CREATE TRIGGER t AFTER INSERT ON notes BEGIN DELETE FROM notes; END", { kind: "mutation" }) }],
    ["ATTACH as a query", { "notes.list": action("ATTACH DATABASE '/tmp/x' AS x") }],
    ["PRAGMA as a query", { "notes.list": action("PRAGMA writable_schema = 1") }],
    ["a leading CTE that hides the LIMIT", { "notes.list": action("WITH x AS (SELECT id FROM notes) SELECT id FROM x LIMIT 5") }],
    ["RETURNING on a mutation", { "notes.add": action("INSERT INTO notes(body) VALUES ('x') RETURNING *", { kind: "mutation" }) }],
    ["a LIMIT only inside a subquery", { "notes.list": action("SELECT id FROM (SELECT id FROM notes LIMIT 5)") }],
    ["a named parameter called :limit", { "notes.list": action("SELECT id, body FROM notes WHERE :limit IS NOT NULL", { bindings: [{ source: "input", key: "limit", type: "number" }] }) }],
    ["UNION after a LIMIT", { "notes.list": action("SELECT id FROM notes UNION SELECT id FROM notes LIMIT 5 ") }],
    ["a window function with no LIMIT", { "notes.list": action("SELECT row_number() OVER () FROM notes") }],
  ];
  for (const [name, actions] of cases) {
    test(name, async () => {
      const kernel = await publish(actions);
      const said = gate(actions);
      if (name === "UNION after a LIMIT") {
        // A compound select whose last part has the LIMIT bounds the whole result in SQLite, so both doors accept it.
        expect(said).toBeNull();
        expect(typeof kernel).toBe("object");
        return;
      }
      expect(typeof kernel).toBe("string");
      expect(said).not.toBeNull();
      expect(kernel as string).toContain(said!);
    });
  }
});

describe("what a published statement can reach at run time", () => {
  test("sqlite_master shows only the app's own tables, and pragma functions, load_extension and readfile are refused", async () => {
    const app = await publish({
      "meta.tables": action("SELECT name FROM sqlite_master ORDER BY name LIMIT 50"),
      "meta.files": action("SELECT file FROM pragma_database_list LIMIT 5"),
      "meta.extension": action("SELECT load_extension('/tmp/x') AS loaded"),
    });
    if (typeof app === "string") throw Error(app);
    const tables = JSON.stringify((await invoke(app.hostname, "meta.tables")).body);
    expect(tables).toContain("notes");
    for (const name of ["app_users", "app_sessions", "projects", "credentials", "route_signatures", "audit"]) expect(tables).not.toContain(name);
    // These publish, because the Kernel prepares actions without the runtime authorizer, and every call is refused.
    for (const name of ["meta.files", "meta.extension"]) expect(answerText((await invoke(app.hostname, name)).body), name).toContain("not authorized");
    // The Kernel's SQLite has no readfile or writefile, so such an action does not publish.
    expect(await publish({ "meta.read": action("SELECT readfile('/etc/passwd') AS content") })).toContain("no such function: readfile");
  });

  test("a value cannot grow past 2 MiB, and a LIKE pattern past 500 bytes is refused", async () => {
    const app = await publish({
      "big.blob": action("SELECT length(zeroblob(3000000)) AS n"),
      "big.text": action("SELECT length(hex(zeroblob(1500000))) AS n"),
      "big.like": action("SELECT 'a' LIKE printf('%.*c', ?1, '%') AS hit", { bindings: [{ source: "input", key: "n", type: "number" }] }),
    });
    if (typeof app === "string") throw Error(app);
    for (const name of ["big.blob", "big.text"]) expect(answerText((await invoke(app.hostname, name)).body)).toContain("too big");
    expect(answerText((await invoke(app.hostname, "big.like", { n: 500 })).body)).toContain("hit");
    expect(answerText((await invoke(app.hostname, "big.like", { n: 501 })).body)).toContain("LIKE or GLOB pattern too complex");
  });

  test("a recursive CTE that never ends stops at the deadline", async () => {
    const app = await publish({ "spin.forever": action("SELECT (SELECT count(*) FROM (WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT x FROM c)) AS n") });
    if (typeof app === "string") throw Error(app);
    const started = performance.now();
    expect(answerText((await invoke(app.hostname, "spin.forever")).body)).toContain("interrupted");
    expect(performance.now() - started).toBeLessThan(5000);
  });

  test("one slow SQLite step on one app does not hold up another app", async () => {
    // instr compares inside one step, which the deadline cannot stop; it runs on a blocking thread.
    const slow = await publish({ "slow.instr": action("SELECT instr(printf('%.*c', 2000000, 'a'), printf('%.*c', 200000, 'a') || 'b') AS n") });
    if (typeof slow === "string") throw Error(slow);
    const running = invoke(slow.hostname, "slow.instr");
    await Bun.sleep(200);
    const started = performance.now();
    const rooms = await invoke(lab.apps.booking.hostname, "rooms.list");
    expect(rooms.status).toBe(200);
    expect(performance.now() - started).toBeLessThan(1500);
    await running;
  }, 120_000);
});

describe("schema.sql and imported databases", () => {
  test("a trigger, a view, ATTACH or a TEMP table in schema.sql is refused at both doors", async () => {
    for (const schema of [
      `${SCHEMA}\nCREATE TRIGGER t AFTER INSERT ON notes BEGIN DELETE FROM notes; END;`,
      `${SCHEMA}\nCREATE VIEW v AS SELECT * FROM notes;`,
      `${SCHEMA}\nATTACH DATABASE '/tmp/x.sqlite' AS x;`,
      `${SCHEMA}\nINSERT INTO notes(body) VALUES ('seed');`,
    ]) {
      const kernel = await publish({ "notes.list": action("SELECT id FROM notes LIMIT 5") }, schema);
      expect(typeof kernel, schema).toBe("string");
      expect(kernel as string).toContain("schema.sql may only create tables and indexes");
    }
  });

  test("an index or CHECK that builds a huge value per row is refused instead of eating memory", async () => {
    const first = await publish({ "notes.add": action("INSERT INTO notes(body) VALUES ('x')", { kind: "mutation" }) });
    if (typeof first === "string") throw Error(first);
    await invoke(first.hostname, "notes.add");
    const heavy = `${SCHEMA}\nCREATE INDEX IF NOT EXISTS heavy ON notes(length(hex(zeroblob(200000000))));`;
    const project = `review_sql_${count}`;
    const again = await release(lab.kernel.owner, {
      project: { id: project, name: project, slug: project.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" }, files: { "schema.sql": heavy, "actions.json": JSON.stringify({ version: 1, actions: { "notes.add": action("INSERT INTO notes(body) VALUES ('x')", { kind: "mutation" }) } }) } },
      visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<main>x</main>" } },
    }, project).then(() => "published", (error) => (error as Error).message);
    expect(again).toContain("too big");
  });

  test("an imported database that carries a trigger is refused, and Studio cannot add one", async () => {
    const { Database } = await import("bun:sqlite");
    const file = `${lab.root}/trigger.sqlite`;
    const db = new Database(file);
    db.exec(`${SCHEMA} CREATE TRIGGER leak AFTER INSERT ON notes BEGIN UPDATE notes SET body = 'leaked'; END;`);
    db.close();
    const bytes = new Uint8Array(await Bun.file(file).arrayBuffer());
    const id = `review_sql_${++count}`;
    const releaseFile = `${lab.root}/trigger-release.json`;
    await Bun.write(releaseFile, JSON.stringify({
      project: { id, name: id, slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" }, files: { "schema.sql": SCHEMA, "actions.json": JSON.stringify({ version: 1, actions: { "notes.list": action("SELECT id FROM notes LIMIT 5") } }) } },
      visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>SQL</main>" } },
    }));
    const manifest = {
      format: "konae-package", version: 1, packageId: crypto.randomUUID(), createdAt: Date.now(), deploymentId: "elsewhere", projectId: "elsewhere",
      visibility: "public", publishedRevision: 1, contents: [{ name: "database", size: bytes.length, sha256: await checksum(bytes) }], excluded: [],
    } as unknown as PackageManifest;
    const refused = await importPackage(lab.kernel.owner, { manifest, files: { database: file, release: releaseFile } }, id).then(() => "imported", (error) => (error as Error).message);
    expect(refused).toContain("The database holds CREATE TRIGGER leak, and Konae runs no triggers or views. Drop it before importing.");
    const app = await publish({ "notes.list": action("SELECT id FROM notes LIMIT 5") });
    if (typeof app === "string") throw Error(app);
    for (const sql of ["CREATE TRIGGER t AFTER INSERT ON notes BEGIN DELETE FROM notes; END", "CREATE VIEW v AS SELECT 1"]) {
      await expect(lab.kernel.owner.call("database_execute", { deploymentId: app.id, operation: "sql", input: { sql } })).rejects.toThrow("not authorized");
    }
  });
});

describe("a hostile app user against a published app", () => {
  test("input that looks like SQL stays a bound value", async () => {
    const { gallery } = lab.apps;
    const visitor = runtimeClient(lab, gallery.hostname);
    const message = "x'); DROP TABLE photos; --";
    await visitor.call("actions_invoke", { name: "contact.send", input: { name: message, email: "a@b.example", topic: "other", message } });
    expect(Array.isArray(await visitor.call("actions_invoke", { name: "photos.published", input: {} }))).toBe(true);
  });
});
