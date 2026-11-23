import { describe, expect, test } from "bun:test";
import initSqlJs, { type SqlJsStatic } from "sql.js";
import { starterProject } from "../lib/starter";
import { defaultSessionConfig } from "./catalog";
import { createDurableKonaeAgent } from "./durable-engine";
import { emptyAgentKeyStore } from "./key-store";

let sql: Promise<SqlJsStatic> | undefined;
const loadSql = () => (sql ??= initSqlJs());

describe("durable Apply gate", () => {
  test("keeps a recovered function candidate blocked when preparation is pending", async () => {
    const candidate = {
      ...starterProject.files,
      "functions.json": JSON.stringify({ version: 1, functions: { main: { entry: "functions/main.ts", mount: "/api", methods: ["GET"], access: "anonymous", actions: [] } } }),
      "functions/main.ts": "export const handle = () => new Response('ok');",
    };
    const agent = createDurableKonaeAgent(starterProject, defaultSessionConfig(), emptyAgentKeyStore(), {
      recoveredFiles: candidate,
      loadSql,
      releaseFacts: async () => ({ modules: { functions: { on: true }, auth: { on: true }, files: { on: true } } }),
      functionPreparation: async () => { throw new Error("owner MCP disconnected"); },
    });
    try {
      await agent.initialize();
      expect(agent.proposal()?.validation).toMatchObject({ ok: true, functionPreparation: { status: "pending" } });
      expect(agent.applyGateStatus()).toMatchObject({ state: "blocked" });
    } finally {
      agent.dispose();
    }
  });
});
