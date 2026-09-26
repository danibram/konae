// `bun run littlebox -- conform`: runs the Action Contract's checks against a
// running Kernel, the way a replacement backend would be held to it.
import { readFile } from "node:fs/promises";
import { McpClient, McpError } from "../src/mcp/client";
import { checkActionContract, type ActionContract } from "../src/lib/action-contract";
import { formatConformanceReport, runConformance, type ConformanceTarget, type RouteRequest } from "../src/lib/action-conformance";

export type KernelTargetOptions = {
  /** The Deployment's URL, e.g. http://notes-ab12.localhost:8787. */
  url: string;
  /** An app session token for a signed-in person. */
  token?: string;
  /** An API key of this Deployment. */
  key?: string;
  secrets?: Record<string, string>;
  /** Replaces fetch, e.g. to add headers a test proxy needs. */
  fetch?: typeof fetch;
};

/**
 * A ConformanceTarget for a Kernel. A `*.localhost` host is reached on
 * 127.0.0.1 with its name in the Host header, as a browser would.
 */
export function kernelTarget(options: KernelTargetOptions): ConformanceTarget {
  const deployment = new URL(options.url);
  const connect = new URL(deployment);
  if (deployment.hostname.endsWith(".localhost")) connect.hostname = "127.0.0.1";
  const send = options.fetch ?? fetch;
  const hostFetch: typeof fetch = (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("host", deployment.host);
    return send(input, { ...init, headers });
  };
  const endpoint = new URL("/_littlebox/mcp", connect).href;
  const visitor = new McpClient(endpoint, { fetch: hostFetch });
  const session = options.token ? new McpClient(endpoint, { fetch: hostFetch, token: options.token }) : visitor;
  return {
    hasSession: Boolean(options.token),
    key: options.key,
    secrets: options.secrets,
    async invoke(name, input, as) {
      try {
        return { ok: true, result: await (as === "session" ? session : visitor).call("actions_invoke", { name, input }) };
      } catch (error) {
        if (error instanceof McpError) return { ok: false, message: error.message };
        throw error;
      }
    },
    async route(request: RouteRequest) {
      const target = new URL(request.path, connect);
      for (const [key, value] of Object.entries(request.query ?? {})) target.searchParams.set(key, value);
      const headers = new Headers(request.headers);
      if (request.session && options.token) {
        // The cookie a browser on the app's own origin would send.
        const cookie = deployment.protocol === "https:" ? "__Host-littlebox_app_session" : "littlebox_app_session";
        headers.set("cookie", `${cookie}=${options.token}`);
        headers.set("sec-fetch-site", "same-origin");
      }
      const response = await hostFetch(target.href, { method: request.method, headers, body: request.body });
      const text = await response.text();
      let body: unknown = text;
      try { body = JSON.parse(text); } catch {}
      return { status: response.status, body };
    },
  };
}

/** Signs in to the Deployment and returns the app session token from its cookie. */
async function appSession(url: string, email: string, password: string): Promise<string> {
  const deployment = new URL(url);
  const connect = new URL("/_littlebox/auth/login", deployment);
  if (deployment.hostname.endsWith(".localhost")) connect.hostname = "127.0.0.1";
  const response = await fetch(connect, {
    method: "POST", headers: { host: deployment.host, "content-type": "application/json" }, body: JSON.stringify({ email, password }),
  });
  if (!response.ok) throw Error(`App sign-in failed: ${(await response.json().catch(() => ({}))).error ?? response.status}.`);
  const token = response.headers.get("set-cookie")?.split(";")[0].split("=").slice(1).join("=");
  if (!token) throw Error("App sign-in returned no session.");
  return token;
}

/** Reads the options, runs the checks, prints one line per check and returns the exit code. */
export async function conformFromCli(args: string[]): Promise<number> {
  const values = (name: string) => args.flatMap((arg, index) => arg === `--${name}` ? [args[index + 1]] : []).filter((value) => value !== undefined && !value.startsWith("--"));
  const one = (name: string) => values(name)[0];
  const url = one("url");
  const file = one("contract");
  if (!url || !file) throw Error("conform needs --url <the Deployment's URL> and --contract <action-contract.json>.");
  const contract = JSON.parse(await readFile(file, "utf8")) as ActionContract & { $schema?: string };
  const problems = checkActionContract(contract);
  if (problems.length > 0) throw Error(`${file} is not an Action Contract: ${problems.slice(0, 3).join("; ")}`);
  const secrets: Record<string, string> = {};
  for (const pair of values("secret")) {
    const at = pair.indexOf("=");
    if (at < 1) throw Error("--secret takes <name>=<value>.");
    secrets[pair.slice(0, at)] = pair.slice(at + 1);
  }
  let token = one("token") ?? process.env.LITTLEBOX_APP_TOKEN;
  if (!token && process.env.LITTLEBOX_APP_EMAIL && process.env.LITTLEBOX_APP_PASSWORD) {
    token = await appSession(url, process.env.LITTLEBOX_APP_EMAIL, process.env.LITTLEBOX_APP_PASSWORD);
  }
  const inputsFile = one("inputs");
  const inputs = inputsFile ? JSON.parse(await readFile(inputsFile, "utf8")) as Record<string, Record<string, unknown>> : undefined;
  const report = await runConformance(contract, kernelTarget({ url, token, key: one("key") ?? process.env.LITTLEBOX_API_KEY, secrets }), { inputs });
  console.log(formatConformanceReport(report));
  return report.failed > 0 ? 1 : 0;
}
