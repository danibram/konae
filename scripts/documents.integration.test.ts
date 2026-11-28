import { expect, test } from "bun:test";
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
if [ "$1" = compile ]; then
  if grep -q hold-first "$6"; then
    touch ${shellWord(path.join(root, "first-render-started"))}
    while [ ! -e ${shellWord(release)} ]; do sleep 0.05; done
  fi
  if grep -q second-render "$6"; then
    touch ${shellWord(path.join(root, "second-render-started"))}
  fi
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
            statement: "INSERT INTO invoices (id, owner_id, issued) VALUES (?1, ?2, 1)",
            bindings: [{ source: "input", key: "id", type: "string" }, { source: "actor", field: "id" }],
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
      const download = (key: string, cookie: string) =>
        fetch(origin + "/_konae/documents/" + key, { headers: { cookie } });
      return { post, download };
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
      expect(cookie).toBeTruthy();
      return cookie!;
    };
    const memberA = await signup(appA);
    const memberB = await signup(appB);
    const issue = async (app: typeof appA, cookie: string, id: string) => {
      const response = await app.post("/_konae/mcp", {
        jsonrpc: "2.0", id: 1, method: "tools/call",
        params: { name: "actions_invoke", arguments: { name: "invoices.issue", input: { id } } },
      }, cookie);
      expect(response.status, await response.clone().text()).toBe(200);
      const body = await response.json() as { result?: { isError: boolean } };
      expect(body.result?.isError, JSON.stringify(body)).toBe(false);
    };
    const ready = async (app: typeof appA, cookie: string, key: string) => {
      const download = await app.download(key, cookie);
      return (await download.text()).startsWith("%PDF-") && download.ok;
    };
    await issue(appA, memberA, "hold-first");
    await until(() => Bun.file(path.join(root, "first-render-started")).exists(), "The first Typst process should start.");
    await issue(appA, memberA, "second-render");
    await issue(appB, memberB, "other");
    await until(() => ready(appB, memberB, "other"), "App B should serve its PDF while App A holds its render slot.");
    expect(await Bun.file(path.join(root, "second-render-started")).exists()).toBe(false);
    expect((await appA.download("hold-first", memberA)).status).toBe(404);
    await writeFile(release, "");
    await until(
      async () => await ready(appA, memberA, "hold-first") && await ready(appA, memberA, "second-render"),
      "App A should serve both PDFs after the held renderer exits.",
    );
  } finally {
    await writeFile(release, "");
    child?.kill();
    await child?.exited;
    await rm(root, { recursive: true, force: true });
  }
}, 45_000);
