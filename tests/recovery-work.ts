import { Database } from "bun:sqlite";
import { expect } from "bun:test";
import { copyFileSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { createServer, connect } from "node:net";
import path from "node:path";
import { pathToFileURL } from "node:url";

type Access = "seed" | "observe" | "frozen";

function query<T>(file: string, readonly: boolean, run: (db: Database) => T) {
  const db = new Database(file, readonly ? { readonly: true } : { readwrite: true });
  try {
    db.run("PRAGMA query_only=ON");
    return run(db);
  } finally { db.close(true); }
}

// macOS SQLite can refuse a read-only WAL database when its shared-memory file is absent.
// Copy the journals too. SQLite rebuilds the WAL index in the scratch directory.
function readonlySnapshot<T>(file: string, run: (db: Database) => T) {
  const suffixes = ["", "-wal", "-journal"];
  const versions = () => suffixes.map(suffix => {
    const st = statSync(file + suffix, { bigint: true, throwIfNoEntry: false });
    return st && `${st.ino}:${st.ctimeNs}:${st.size}`;
  });
  const before = versions();
  if (!before[0]) throw Error(`Missing SQLite database: ${file}`);
  const dir = mkdtempSync(path.join(tmpdir(), "konae-sqlite-read-"));
  try {
    const snapshot = path.join(dir, "snapshot.sqlite");
    try {
      suffixes.forEach((suffix, i) => {
        if (before[i]) copyFileSync(file + suffix, snapshot + suffix);
      });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
      throw error;
    }
    if (versions().every((value, i) => value === before[i])) return { value: query(snapshot, false, run) };
  } finally { rmSync(dir, { recursive: true, force: true }); }
}

function use<T>(file: string, access: Access, run: (db: Database) => T, deadline = performance.now() + 5000): T {
  if (access === "seed") {
    const db = new Database(file, { readwrite: true, create: true });
    try { return run(db); } finally { db.close(true); }
  }
  if (access === "frozen") return query(pathToFileURL(file).href + "?immutable=1", true, run);
  for (;;) {
    try {
      return query(file, true, run);
    } catch (error) {
      const code = (error as { code?: string }).code;
      if (code === "SQLITE_CANTOPEN") {
        const snapshot = readonlySnapshot(file, run);
        if (snapshot) return snapshot.value;
      } else if (code !== "SQLITE_BUSY" && !code?.startsWith("SQLITE_BUSY_")) {
        throw error;
      }
      const remaining = deadline - performance.now();
      if (remaining <= 0) throw error;
      Bun.sleepSync(Math.min(50, Math.ceil(remaining)));
    }
  }
}

// The receiver commits the effect and drops the connection before acknowledging it.
async function loseAcknowledgement(receipts: string, callId: string) {
  const server = createServer(socket => socket.once("data", bytes => {
    use(receipts, "seed", db => {
      db.run("CREATE TABLE receipts(call_id TEXT PRIMARY KEY)");
      db.run("INSERT INTO receipts VALUES(?)", [bytes.toString()]);
    });
    socket.destroy();
  }));
  await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const { port } = server.address() as { port: number };
    const received = await new Promise<number>((resolve, reject) => {
      const socket = connect(port, "127.0.0.1", () => socket.write(callId));
      let bytes = 0;
      socket.on("data", chunk => { bytes += chunk.length; });
      socket.on("end", () => resolve(bytes));
      socket.on("error", reject);
    });
    expect(received).toBe(0);
  } finally { await new Promise<void>(resolve => server.close(() => resolve())); }
}

export async function seedWork(root: string, id: string) {
  const metadata = path.join(root, "apps", id, ".konae");
  await Bun.write(path.join(metadata, "domain.json"), JSON.stringify({
    hostname: "alias.lab.test", token: "fixture", verified: true, verifiedAt: 1, createdAt: 1,
  }));
  await Bun.write(path.join(metadata, "hostname.json"), JSON.stringify({ previous: ["old.lab.test"] }));
  const later = Date.now() + 86_400_000;
  const schema = await Bun.file(path.resolve(import.meta.dir, "../kernel-rs/kernel/src/calls.sql")).text();
  use(path.join(root, "calls.sqlite"), "seed", db => {
    db.exec(schema);
    for (const [call, state, attempts, lease, thenState] of [
      ["unsent", "pending", 0, null, null], ["ack-lost", "pending", 1, later, null],
      ["finished", "delivered", 1, null, "ok"], ["continuing", "delivered", 1, later, "processing"],
      ["refused", "failed", 3, null, null],
    ] as const) {
      db.run(`INSERT INTO calls(id,deployment,action,input,state,attempts,next_attempt_at,leased_until,
        then_state,created_at,updated_at) VALUES(?,?,'hook.send','{}',?,?,?,?,?,1,1)`,
        [call, id, state, attempts, lease ? 0 : later, lease, thenState]);
    }
  });
  const claimed = use(path.join(root, "calls.sqlite"), "observe", db =>
    db.query<{id:string}, []>("SELECT id FROM calls WHERE state='pending' AND leased_until IS NOT NULL").get());
  expect(claimed?.id).toBe("ack-lost");
  await loseAcknowledgement(path.join(root, "../receiver.sqlite"), claimed!.id);
  use(path.join(root, "mail.sqlite"), "seed", db => {
    for (const state of ["pending", "accepted", "delivered", "failed"]) {
      db.run(`INSERT INTO jobs(id,purpose,recipient,scope,app_name,token_hash,link_expires_at,state,
        next_attempt_at,created_at,updated_at) VALUES(?,'notify','receiver@lab.test',?,'Lab','lab',?,?,?,1,1)`,
        [state, "deployment:" + id, later, state, later]);
    }
  });
  use(path.join(root, "mcp.sqlite"), "seed", db => {
    db.run("CREATE TABLE IF NOT EXISTS oauth_codes(hash TEXT PRIMARY KEY, body TEXT NOT NULL, expires INTEGER NOT NULL)");
    db.run("INSERT INTO credentials(hash,owner,scopes,projects,expires) SELECT 'b07',owner,'[]','[]',? FROM projects WHERE id=?", [later,id]);
    db.run("INSERT INTO oauth_codes VALUES('b07','{}',?)", [later]);
    db.run("INSERT INTO plans(id,owner,project,revision,transfer,expires) SELECT 'b07',owner,id,0,'b07',? FROM projects WHERE id=?", [later,id]);
  });
  use(path.join(root, "google.sqlite"), "seed", db => {
    db.run(`CREATE TABLE google_flows(state_hash TEXT PRIMARY KEY, browser_hash TEXT NOT NULL,
      deployment_id TEXT NOT NULL, app_origin TEXT NOT NULL, nonce TEXT NOT NULL, verifier TEXT NOT NULL,
      intent TEXT NOT NULL, intent_value TEXT NOT NULL DEFAULT '', expires_at INTEGER NOT NULL)`);
    db.run("INSERT INTO google_flows VALUES('b07','browser',?,'http://lab.test','nonce','verifier','login','',?)", [id, later]);
  });
  const reset = "ea".repeat(32);
  use(path.join(root, "apps", id, "data/identity.sqlite"), "seed", db => {
    const hash = new Bun.CryptoHasher("sha256").update(reset).digest("hex");
    db.run(`INSERT INTO account_challenges(token_hash,user_id,email,purpose,scope,created_at,expires_at)
            SELECT ?,id,email,'recovery',?,1,2 FROM app_users WHERE email='writer@copy.test'`, [hash, "deployment:" + id]);
  });
  use(path.join(root, "apps", id, "data/usage.sqlite"), "seed", db => {
    db.run("INSERT OR REPLACE INTO schedule_state VALUES('notes.tick','daily 10:00',0)");
    db.run("INSERT INTO scheduled_runs(action,at,outcome,changes) VALUES('notes.tick',?,'ok',1)", [Date.now()]);
  });
  return reset;
}

export function checkWork(root: string, id: string, restored: boolean, access: Access = "observe") {
  const rows = (file: string, sql: string) => use(path.join(root,file), access, db => db.query(sql).values());
  expect(rows("calls.sqlite", "SELECT id,state,attempts,then_state FROM calls ORDER BY id")).toEqual([
    ["ack-lost", restored ? "failed" : "pending", 1, null],
    ["continuing","delivered",1,restored ? "interrupted" : "processing"],
    ["finished","delivered",1,"ok"], ["refused","failed",3,null],
    ["unsent",restored ? "failed" : "pending",0,null],
  ]);
  expect(rows("calls.sqlite", "SELECT count(*) FROM calls WHERE error='restored-outcome-unknown' AND leased_until IS NULL")).toEqual([[restored ? 2 : 0]]);
  expect(rows("mail.sqlite", "SELECT id,state FROM jobs WHERE id IN ('accepted','delivered','failed','pending') ORDER BY id"))
    .toEqual([["accepted","accepted"],["delivered","delivered"],["failed","failed"],["pending",restored ? "failed" : "pending"]]);
  expect(rows("mcp.sqlite", "SELECT count(*) FROM oauth_codes WHERE hash='b07'")).toEqual([[restored ? 0 : 1]]);
  expect(rows("mcp.sqlite", "SELECT count(*) FROM plans WHERE id='b07' AND receipt IS NULL")).toEqual([[restored ? 0 : 1]]);
  const completed = rows("mcp.sqlite", "SELECT count(*) FROM plans WHERE receipt IS NOT NULL")[0][0];
  if (restored) expect(completed).toBe(0);
  else expect(Number(completed)).toBeGreaterThan(0);
  expect(rows("mcp.sqlite", "SELECT count(*) FROM credentials WHERE hash='b07'")).toEqual([[restored ? 0 : 1]]);
  const identity = `apps/${id}/data/identity.sqlite`;
  expect(rows(identity, "SELECT count(*) FROM account_challenges WHERE purpose='recovery' AND expires_at=2")).toEqual([[restored ? 0 : 1]]);
  expect(rows(identity, "SELECT count(*) FROM entry_links")).toEqual([[restored ? 0 : 1]]);
  const transfers = rows("transfers.sqlite", "SELECT count(*) FROM transfers")[0][0] as number;
  if (restored) expect(transfers).toBe(0);
  else expect(transfers).toBeGreaterThan(0);
  const usage = `apps/${id}/data/usage.sqlite`;
  expect(rows(usage, "SELECT slot FROM schedule_state WHERE action='notes.tick'")).toEqual(restored ? [] : [[0]]);
  expect(rows(usage, "SELECT count(*) FROM scheduled_runs WHERE action='notes.tick'")).toEqual([[1]]);
}

export function checkReceiver(root: string) {
  expect(use(path.join(root, "../receiver.sqlite"), "observe", db => db.query("SELECT call_id FROM receipts").values())).toEqual([["ack-lost"]]);
}

export function checkRevocations(root: string, id: string, reconciled: boolean, access: Access = "observe") {
  use(path.join(root, "apps", id, "data/identity.sqlite"), access, db => {
    expect(db.query("SELECT state FROM app_users WHERE email='retired@copy.test'").values())
      .toEqual([[reconciled ? "suspended" : "active"]]);
    expect(db.query("SELECT name, revoked_at IS NOT NULL FROM api_keys ORDER BY name").values())
      .toEqual([["kept", 0], ["retired", Number(reconciled)]]);
  });
}

export async function checkWorkers(root: string, id: string) {
  const data = path.join(root, "apps", id, "data");
  const deadline = performance.now() + 5000;
  const slot = () => use(path.join(data, "usage.sqlite"), "observe", db =>
    db.query("SELECT slot FROM schedule_state WHERE action='notes.tick'").values(), deadline);
  let slots = slot();
  while (slots.length === 0 && performance.now() < deadline) {
    await Bun.sleep(100);
    slots = slot();
  }
  expect(Number(slots[0]?.[0])).toBeGreaterThan(0);
  expect(use(path.join(data, "database.sqlite"), "observe", db =>
    db.query("SELECT count(*) FROM notes WHERE user_id='timer'").values())).toEqual([[0]]);
  expect(use(path.join(root, "calls.sqlite"), "observe", db =>
    db.query("SELECT id,attempts FROM calls WHERE error='restored-outcome-unknown' ORDER BY id").values()))
    .toEqual([["ack-lost",1],["unsent",0]]);
  expect(use(path.join(root, "mail.sqlite"), "observe", db =>
    db.query("SELECT attempts,error_kind FROM jobs WHERE id='pending'").values())).toEqual([[0,"restored"]]);
}
