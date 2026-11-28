import { expect, test } from "bun:test";
import { chmod, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildLocal, initLocal } from "./konae";

const binary = process.env.KONAE_KERNEL_BIN;
const curl = Bun.which("curl");
const openssl = Bun.which("openssl");
const integration = binary && curl && openssl ? test : test.skip;

function freePorts() {
  const servers = Array.from({ length: 3 }, () =>
    Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response() }));
  const ports = servers.map((server) => server.port);
  for (const server of servers) server.stop(true);
  return ports;
}

function object(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw Error("Expected a response object.");
  }
  return Object.fromEntries(Object.entries(value));
}

function requiredText(value: unknown): string {
  if (typeof value === "string" && value.length > 0) return value;
  throw Error("Expected a response string.");
}

integration("a member can upload once through FTPS and only that member can read the file", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-ftps-member-"));
  let server: ReturnType<typeof Bun.spawn> | undefined;
  try {
    const folder = path.join(root, "application");
    const installation = path.join(root, "installation");
    const settings = await initLocal(folder, { name: "FTPS acceptance" });
    settings.visibility = "public";
    settings.signupPolicy = { mode: "open", allowlist: [] };
    settings.modules = {
      ...settings.modules,
      auth: { on: true },
      files: { on: true, fileAccess: "app-users" },
    };
    await writeFile(path.join(folder, "konae.json"), JSON.stringify(settings));
    await writeFile(path.join(folder, "app/ftps.json"), JSON.stringify({
      version: 1, destinations: { photos: { kind: "member" } },
    }));
    await buildLocal(folder);
    const apps = path.join(installation, "apps");
    await mkdir(apps, { recursive: true });
    await symlink(folder, path.join(apps, settings.id), "dir");

    const [httpPort, ftpsPort, passivePort] = freePorts();
    const certificate = path.join(installation, "cert.pem");
    const certificateConfig = path.join(installation, "openssl.cnf");
    await writeFile(certificateConfig, "[req]\ndistinguished_name=dn\nx509_extensions=leaf\nprompt=no\n[dn]\nCN=127.0.0.1\n[leaf]\nsubjectAltName=IP:127.0.0.1\n");
    const key = path.join(installation, "key.pem");
    const generated = Bun.spawnSync([openssl!, "req", "-x509", "-newkey", "rsa:2048", "-nodes",
      "-days", "1", "-config", certificateConfig, "-keyout", key, "-out", certificate]);
    expect(generated.exitCode, generated.stderr.toString()).toBe(0);
    await chmod(key, 0o600);
    await writeFile(path.join(installation, "ftps.json"), JSON.stringify({
      bind: `127.0.0.1:${ftpsPort}`, passivePorts: [passivePort, passivePort],
      advertisedHost: "127.0.0.1", advertisedPort: ftpsPort,
      certificate: "cert.pem", privateKey: "key.pem",
    }), { mode: 0o600 });
    const origin = `http://${settings.slug}.localhost:${httpPort}`;
    const log = Bun.file(path.join(root, "kernel.log"));
    server = Bun.spawn([binary!], { stdout: log, stderr: log, env: {
      ...process.env,
      KONAE_DATA_DIR: installation, KONAE_BIND: "127.0.0.1",
      KONAE_KERNEL_PORT: String(httpPort), KONAE_HOST_SUFFIX: "localhost",
      KONAE_PUBLIC_ORIGIN: `http://localhost:${httpPort}`, KONAE_ACCOUNT_VERIFICATION: "local",
      KONAE_EXECUTION_MODE: "test", KONAE_MAIL_TRANSPORT: "development",
    } });
    for (let attempt = 0; attempt < 160; attempt++) {
      const ready = await fetch(origin + "/_konae/health").then((response) => response.ok, () => false);
      if (ready) break;
      if (server.exitCode !== null || attempt === 159) throw Error(await log.text());
      await Bun.sleep(50);
    }
    const post = (route: string, payload: object, cookie?: string) => fetch(origin + route, {
      method: "POST", body: JSON.stringify(payload),
      headers: { "content-type": "application/json", origin, ...(cookie ? { cookie } : {}) },
    });
    const join = async (email: string) => {
      const response = await post("/_konae/auth/signup", { email, password: "integration test password" });
      expect(response.status, await response.clone().text()).toBe(200);
      return requiredText(response.headers.get("set-cookie")).split(";", 1)[0];
    };
    const call = async (cookie: string, name: string, args: object) => {
      const response = await post("/_konae/mcp", {
        jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args },
      }, cookie);
      expect(response.status).toBe(200);
      return object(object(await response.json()).result);
    };
    const result = async (cookie: string, name: string, args: object) => {
      const reply = await call(cookie, name, args);
      if (reply.isError === true) throw Error(JSON.stringify(reply));
      return object(reply.structuredContent).result;
    };
    const alice = await join("alice@example.test");
    const bob = await join("bob@example.test");
    const credential = object(await result(alice, "ftps_upload_create", { destination: "photos" }));
    const uploadId = requiredText(credential.uploadId);
    const username = requiredText(credential.username);
    const password = requiredText(credential.password);
    const payload = path.join(root, "sample.txt");
    const bytes = "The file belongs to Alice.";
    await writeFile(payload, bytes);
    const auth = path.join(root, "client.netrc");
    await writeFile(auth, `machine 127.0.0.1 login ${username} password ${password}\n`, { mode: 0o600 });
    const upload = async () => {
      const client = Bun.spawn([curl!, "--fail", "--silent", "--show-error", "--max-time", "5",
        "--ssl-reqd", "--cacert", certificate, "--netrc-file", auth, "--upload-file", payload,
        `ftp://127.0.0.1:${ftpsPort}/photo.txt`], { stdout: "ignore", stderr: "pipe" });
      const error = new Response(client.stderr).text();
      return { code: await client.exited, error: await error };
    };
    const first = await upload();
    expect(first.code, first.error).toBe(0);
    expect((await upload()).code).not.toBe(0);
    const receipt = object(await result(alice, "ftps_upload_status", { uploadId }));
    const fileId = requiredText(receipt.fileId);
    const otherReceipt = await call(bob, "ftps_upload_status", { uploadId });
    expect(JSON.stringify(otherReceipt)).not.toContain(fileId);
    const read = (cookie: string) => fetch(origin + "/_konae/files/" + fileId, { headers: { cookie } });
    const own = await read(alice);
    expect(own.status).toBe(200);
    expect(await own.text()).toBe(bytes);
    expect((await read(bob)).status).toBe(404);
    const ownFiles = await result(alice, "files_list", {});
    const otherFiles = await result(bob, "files_list", {});
    expect(ownFiles).toHaveLength(1);
    expect(otherFiles).toEqual([]);
  } finally {
    if (server) { server.kill(); await server.exited; }
    await rm(root, { recursive: true, force: true });
  }
}, 30_000);
