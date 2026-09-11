import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { planDrizzleDatabaseChange } from "./database-lifecycle/server";

const databaseLifecyclePlugin = {
  name: "littlebox-database-lifecycle",
  configureServer(server: { middlewares: { use: (handler: (request: import("node:http").IncomingMessage, response: import("node:http").ServerResponse, next: () => void) => void) => void } }) {
    server.middlewares.use((request, response, next) => {
      if (request.method !== "POST" || request.url !== "/_littlebox/admin/database-lifecycle/plan") {
        next();
        return;
      }
      void readJsonBody(request)
        .then((body) => planDrizzleDatabaseChange(body as { acceptedSource: string; candidateSource: string }))
        .then((result) => sendJson(response, 200, { result }))
        .catch((error) => sendJson(response, 400, { error: error instanceof Error ? error.message : String(error) }));
    });
  },
};

export default defineConfig({
  plugins: [databaseLifecyclePlugin, react()],
  server: {
    port: 4173,
    proxy: {
      "/_littlebox/admin": process.env.LITTLEBOX_KERNEL_URL ?? "http://localhost:8787",
      "/_littlebox/health": process.env.LITTLEBOX_KERNEL_URL ?? "http://localhost:8787",
    },
  },
});

function readJsonBody(request: import("node:http").IncomingMessage): Promise<unknown> {
  return new Promise((resolve, reject) => {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
      if (body.length > 120_000) reject(new Error("Request body is too large."));
    });
    request.on("end", () => {
      try { resolve(JSON.parse(body)); } catch { reject(new Error("Request body must be JSON.")); }
    });
    request.on("error", reject);
  });
}

function sendJson(response: import("node:http").ServerResponse, status: number, body: unknown) {
  response.statusCode = status;
  response.setHeader("content-type", "application/json");
  response.end(JSON.stringify(body));
}
