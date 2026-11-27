import { expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildLocal, initLocal } from "./konae";

const kernel = process.env.KONAE_KERNEL_BIN;
const typst = Bun.which("typst");
const integration = kernel && typst ? test : test.skip;

integration("Documents serves a second app while one app holds its render slot", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-document-slots-"));
  const installation = path.join(root, "kernel");
  const release = path.join(root, "release-first-render");
  const renderer = path.join(root, "typst");
  const log = path.join(root, "kernel.log");
  let child: ReturnType<typeof Bun.spawn> | undefined;
  let logFile: ReturnType<typeof Bun.file> | undefined;
  async function until(check: () => boolean | Promise<boolean>, description: string) {
    const deadline = Date.now() + 8_000;
    while (Date.now() < deadline) {
      if (await check()) return;
      if (child && child.exitCode !== null) break;
      await Bun.sleep(40);
    }
    throw new Error(`${description}\n${await logFile?.text().catch(() => "") ?? ""}`);
  }

  try {
    const shellWord = (value: string) => "'" + value.replaceAll("'", "'\\''") + "'";
    await writeFile(renderer, `#!/bin/sh
if [ "$1" = compile ] && grep -q hold-first "$6"; then
  touch ${shellWord(path.join(root, "first-render-started"))}
  while [ ! -e ${shellWord(release)} ]; do sleep 0.05; done
fi
exec ${shellWord(typst!)} "$@"
`, { mode: 0o700 });
    const socket = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response() });
    const port = socket.port;
    socket.stop(true);
    const apps = path.join(installation, "apps");
    await mkdir(apps, { recursive: true });

    async function prepare(name: string) {
      const folder = path.join(root, name);
      const settings = await initLocal(folder, { name });
      settings.visibility = "public";
      settings.signupPolicy = { mode: "open", allowlist: [] };
      settings.modules = { ...settings.modules, actions: { on: true }, auth: { on: true }, documents: { on: true } };
      await writeFile(path.join(folder, "konae.json"), JSON.stringify(settings));
      await writeFile(path.join(folder, "app/schema.sql"), "CREATE TABLE IF NOT EXISTS invoices (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, issued INTEGER NOT NULL);");
      await writeFile(path.join(folder, "app/actions.json"), JSON.stringify({
        version: 1,
        actions: {
          "invoices.issue": {
            kind: "mutation", access: "authenticated",
            statement: "UPDATE invoices SET issued = 1 WHERE id = ?1",
            bindings: [{ source: "input", key: "id", type: "string" }],
            issue: { key: "id", download: "invoices.read", snapshot: ["id"], template: "#text(issue.id)" },
          },
          "invoices.read": {
            kind: "query", access: "authenticated",
            statement: "SELECT id FROM invoices WHERE id = ?1 AND owner_id = ?2 LIMIT 1",
            bindings: [{ source: "input", key: "issueKey", type: "string" }, { source: "actor", field: "id" }],
          },
        },
      }));
      await buildLocal(folder, { development: true });
      await symlink(folder, path.join(apps, settings.id), "dir");
      const origin = `http://${settings.slug}.localhost:${port}`;
      const post = (endpoint: string, input: object, cookie?: string) => fetch(origin + endpoint, {
        method: "POST", body: JSON.stringify(input),
        headers: { "content-type": "application/json", origin, ...(cookie ? { cookie } : {}) },
      });
      const data = () => new Database(path.join(folder, "data/database.sqlite"));
      return { origin, post, data };
    }

    const appA = await prepare("Invoices A");
    const appB = await prepare("Invoices B");
    logFile = Bun.file(log);
    child = Bun.spawn([kernel!], {
      env: {
        ...process.env,
        KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_HOST_SUFFIX: "localhost",
        KONAE_DATA_DIR: installation, KONAE_PUBLIC_ORIGIN: `http://localhost:${port}`,
        KONAE_EXECUTION_MODE: "test", KONAE_MAIL_TRANSPORT: "development",
        KONAE_DOCUMENTS_TYPST_BIN: renderer,
      },
      stdout: logFile, stderr: "inherit",
    });
    await until(
      () => fetch(`http://localhost:${port}/_konae/health`).then((response) => response.ok, () => false),
      "The Kernel should start both apps.",
    );

    const signup = async (app: typeof appA) => {
      const credentials = { email: "test@example.test", password: "a long integration password" };
      const response = await app.post("/_konae/auth/signup", credentials);
      expect(response.status, await response.clone().text()).toBe(200);
      const cookie = response.headers.get("set-cookie")?.split(";", 1)[0];
      const { result } = await response.json() as { result: { user: { userId: string } } };
      expect(cookie).toBeTruthy();
      return { cookie: cookie!, id: result.user.userId };
    };
    const memberA = await signup(appA);
    const memberB = await signup(appB);
    const issue = async (app: typeof appA, member: typeof memberA, id: string) => {
      using db = app.data();
      db.run("INSERT INTO invoices (id, owner_id, issued) VALUES (?1, ?2, 0)", [id, member.id]);
      const response = await app.post("/_konae/mcp", {
        jsonrpc: "2.0", id: 1, method: "tools/call",
        params: { name: "actions_invoke", arguments: { name: "invoices.issue", input: { id } } },
      }, member.cookie);
      expect(response.status, await response.clone().text()).toBe(200);
      const body = await response.json() as { result?: { isError: boolean } };
      expect(body.result?.isError, JSON.stringify(body)).toBe(false);
    };
    const status = (app: typeof appA, id: string) => {
      using db = app.data();
      return db.query<{ state: string }, [string]>(
        "SELECT state FROM __konae_documents WHERE issue_key = ?1",
      ).get(id)?.state;
    };

    await issue(appA, memberA, "hold-first");
    await until(() => Bun.file(path.join(root, "first-render-started")).exists(), "The first Typst process should start.");
    await issue(appA, memberA, "second");
    await issue(appB, memberB, "other");
    await until(() => status(appB, "other") === "ready", "App B should finish while App A holds its render slot.");
    expect(status(appA, "hold-first")).toBe("rendering");
    expect(status(appA, "second")).toBe("pending");
    const download = await fetch(`${appB.origin}/_konae/documents/other`, { headers: { cookie: memberB.cookie } });
    expect(download.status).toBe(200);
    expect((await download.text()).slice(0, 5)).toBe("%PDF-");
    await writeFile(release, "");
    await until(
      () => status(appA, "hold-first") === "ready" && status(appA, "second") === "ready",
      "App A should drain its queue after the held renderer exits.",
    );
  } finally {
    await writeFile(release, "");
    child?.kill();
    await child?.exited;
    await rm(root, { recursive: true, force: true });
  }
}, 45_000);
