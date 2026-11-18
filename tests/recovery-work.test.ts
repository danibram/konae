import { Database } from "bun:sqlite";
import { expect, test } from "bun:test";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { checkRevocations } from "./recovery-work";

test("the observer reads committed revocations still in the WAL", () => {
  const scratch = mkdtempSync(path.join(tmpdir(), "konae-observe-wal-"));
  const source = path.join(scratch, "source.sqlite");
  const db = new Database(source, { create: true });
  try {
    db.exec(`PRAGMA journal_mode=WAL;
      PRAGMA wal_autocheckpoint=0;
      CREATE TABLE app_users(email, state);
      CREATE TABLE api_keys(name, revoked_at);
      INSERT INTO app_users VALUES ('retired@copy.test','active');
      INSERT INTO api_keys VALUES ('kept',NULL);
      PRAGMA wal_checkpoint(TRUNCATE);`);
    db.exec("UPDATE app_users SET state='suspended'; INSERT INTO api_keys VALUES ('retired',1)");
    const data = path.join(scratch, "apps/lab/data");
    mkdirSync(data, { recursive: true });
    const identity = path.join(data, "identity.sqlite");
    copyFileSync(source, identity);
    const main = new Database(pathToFileURL(identity).href + "?immutable=1", { readonly: true });
    try {
      expect(main.query("SELECT name FROM api_keys").values()).toEqual([["kept"]]);
    } finally { main.close(); }
    copyFileSync(source + "-wal", identity + "-wal");
    checkRevocations(scratch, "lab", true);
  } finally {
    db.close();
    rmSync(scratch, { recursive: true, force: true });
  }
});
