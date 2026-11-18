import { randomBytes } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import type { Plugin } from "vite";

export function devOwnerPlugin(target: string, credentialsFile?: string): Plugin {
  return {
    name: "konae-dev-owner",
    apply: "serve",
    configureServer(server) {
      if (!credentialsFile) return;
      const upstream = new URL(target);
      if (upstream.protocol !== "http:" || !["localhost", "127.0.0.1", "[::1]"].includes(upstream.hostname)) {
        throw new Error("Development owner sign-in requires a Kernel on localhost. Set KONAE_DEV_AUTO_LOGIN=0 to sign in manually.");
      }
      if (server.config.server.host !== "127.0.0.1") {
        throw new Error("Development owner sign-in requires the editor to bind to 127.0.0.1.");
      }
      let signingIn: Promise<string> | undefined;

      async function signIn(): Promise<string> {
        let credentials: { email: string; password: string };
        let signup = false;
        try {
          credentials = JSON.parse(await readFile(credentialsFile!, "utf8"));
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
          credentials = { email: "dev@konae.local", password: randomBytes(32).toString("hex") };
          await writeFile(credentialsFile!, JSON.stringify(credentials), { mode: 0o600, flag: "wx" });
          signup = true;
        }
        const authenticate = (create: boolean) => fetch(new URL(`/_konae/owner/${create ? "signup" : "login"}`, upstream), {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ ...credentials, ...(create ? { setupKey: "local-development-key" } : {}) }),
        });
        let response = await authenticate(signup);
        if (!signup && response.status === 401) response = await authenticate(true);
        const cookie = response.headers.get("set-cookie");
        if (!response.ok || !cookie) {
          const body = await response.json().catch(() => ({})) as { error?: string };
          throw new Error(`${body.error ?? "Could not sign in the development owner."} Set KONAE_DEV_AUTO_LOGIN=0 to use the owner sign-in page.`);
        }
        return cookie;
      }

      server.middlewares.use(async (request, response, next) => {
        if (request.method !== "GET" || request.url?.split("?")[0] !== "/_konae/owner/session") return next();
        const host = request.headers.host;
        const port = server.config.server.port;
        const localHost = host === `localhost:${port}` || host === `127.0.0.1:${port}`;
        const origin = request.headers.origin;
        const peer = request.socket.remoteAddress;
        if (!localHost || !["127.0.0.1", "::ffff:127.0.0.1", "::1"].includes(peer ?? "")
          || (origin && origin !== `http://${host}`) || request.headers["sec-fetch-site"] === "cross-site") {
          response.writeHead(403).end();
          return;
        }
        try {
          const sessionUrl = new URL("/_konae/owner/session", upstream);
          const current = await fetch(sessionUrl, { headers: { cookie: request.headers.cookie ?? "" } });
          let body = await current.json() as { signedIn?: boolean };
          if (!current.ok) throw new Error(`The Kernel answered ${current.status} when checking the owner session.`);
          if (!body.signedIn) {
            signingIn ??= signIn().finally(() => { signingIn = undefined; });
            const cookie = await signingIn;
            const signedIn = await fetch(sessionUrl, { headers: { cookie: cookie.split(";")[0] } });
            body = await signedIn.json() as { signedIn?: boolean };
            if (!signedIn.ok || !body.signedIn) throw new Error("The Kernel did not keep the development owner session.");
            response.setHeader("set-cookie", cookie);
          }
          response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
          response.end(JSON.stringify(body));
        } catch (error) {
          response.writeHead(503, { "content-type": "application/json", "cache-control": "no-store" });
          response.end(JSON.stringify({ error: error instanceof Error ? error.message : "Development owner sign-in failed." }));
        }
      });
    },
  };
}
