// In the spike of ADR-0027 decision 7, the Kernel's checks, built from
// kernel-rs/checks for wasm32-unknown-unknown, answer the shared case files
// as the Validation Gate does today. The Gate does not call them yet.
import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import initSqlJs from "sql.js";
import { createCapsuleValidator } from "../agent/gate";
import { starterProject } from "../lib/starter";
import { parseServerActionSource } from "./actions";
import { expectedOutcome, outcomeOf } from "./refusal-cases";

/** A refusal as the case runners compare it. A file that does not parse has no code. */
type Outcome = { code: string | null; message: string } | null;

const crate = new URL("../../kernel-rs/checks/", import.meta.url).pathname;
const wasmPath = join(crate, "target/wasm32-unknown-unknown/release/konae_checks.wasm");

/** Builds the module when this machine has Rust's wasm32-unknown-unknown target, else answers why not. */
function build(): string | null {
  const sysroot = Bun.spawnSync(["rustc", "--print", "sysroot"], { stdout: "pipe", stderr: "pipe" });
  if (!sysroot.success) return "rustc is not installed";
  if (!existsSync(join(sysroot.stdout.toString().trim(), "lib/rustlib/wasm32-unknown-unknown"))) {
    return "rustup target add wasm32-unknown-unknown has not run";
  }
  const cargo = Bun.spawnSync(["cargo", "build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path", join(crate, "Cargo.toml")], {
    stdout: "pipe",
    stderr: "pipe",
  });
  if (!cargo.success) throw new Error(`kernel-rs/checks did not build:\n${cargo.stderr.toString()}`);
  return null;
}

const unavailable = build();

type Checks = { memory: WebAssembly.Memory; alloc(len: number): number; dealloc(ptr: number, len: number): void } & Record<
  "check_actions" | "check_schema",
  (ptr: number, len: number) => bigint
>;

function load(): { module: WebAssembly.Module; checks: Checks } {
  const module = new WebAssembly.Module(readFileSync(wasmPath));
  return { module, checks: new WebAssembly.Instance(module, {}).exports as unknown as Checks };
}

/** Calls one check with `text`, and answers its refusal as the case runners compare it. */
function call(checks: Checks, check: "check_actions" | "check_schema", text: string): Outcome {
  const input = new TextEncoder().encode(text);
  const ptr = checks.alloc(input.length);
  new Uint8Array(checks.memory.buffer, ptr, input.length).set(input);
  const packed = checks[check](ptr, input.length);
  checks.dealloc(ptr, input.length);
  const answerPtr = Number(packed >> 32n);
  const answerLen = Number(packed & 0xffffffffn);
  const answer = JSON.parse(new TextDecoder().decode(new Uint8Array(checks.memory.buffer, answerPtr, answerLen)));
  checks.dealloc(answerPtr, answerLen);
  return answer === null ? null : { code: answer.code, message: answer.message };
}

type Case = { case: string; actions?: Record<string, unknown>; schema?: string; error: string | null; code?: string };

describe.skipIf(unavailable !== null)(`the Kernel's checks as WebAssembly${unavailable ? ` (skipped: ${unavailable})` : ""}`, () => {
  test("import nothing, so they reach no file, clock or network", () => {
    expect(WebAssembly.Module.imports(load().module)).toEqual([]);
  });

  for (const file of ["sql-rule-cases.json", "guard-cases.json"]) {
    test(`refuse every case in kernel-rs/src/${file} as the Gate does`, async () => {
      const cases: Case[] = await Bun.file(new URL(`../../kernel-rs/src/${file}`, import.meta.url)).json();
      expect(cases.length).toBeGreaterThanOrEqual(8);
      const { checks } = load();
      const gate = createCapsuleValidator(() => initSqlJs());
      for (const item of cases) {
        let kernel: Outcome;
        let editor: Outcome;
        if (item.schema !== undefined) {
          kernel = call(checks, "check_schema", item.schema);
          const result = await gate.validate({ ...starterProject.files, "schema.sql": item.schema, "actions.json": JSON.stringify({ version: 1, actions: {} }) });
          editor = result.errors[0] ? { code: result.errors[0].code ?? "(no code)", message: result.errors[0].message } : null;
        } else {
          const source = JSON.stringify({ version: 1, actions: item.actions });
          kernel = call(checks, "check_actions", source);
          editor = outcomeOf(() => parseServerActionSource(source));
        }
        expect(kernel, item.case).toEqual(editor);
        expect(kernel, item.case).toEqual(expectedOutcome(item));
      }
    });
  }

  // Where the two doors differ today. The Kernel reads actions.json into
  // typed fields first, so a value of the wrong type stops at that reading
  // without a code. Its schema check reads tokens only, and the authorizer
  // refuses the rest once SQLite applies the schema.
  test("differ from the Gate on a field of the wrong type and on what only the authorizer refuses", async () => {
    const { checks } = load();
    const query = { kind: "query", access: "anonymous", statement: "SELECT 1 FROM t LIMIT 1", bindings: [] as unknown[] };
    const typed: [Record<string, unknown>, Outcome][] = [
      [{ kind: "query", access: "anonymous", statement: "SELECT 1 FROM t LIMIT 1" }, { code: "(not a refusal)", message: "Action a.b requires a bindings array." }],
      [{ ...query, statement: 5 }, { code: "(not a refusal)", message: "Invalid server action: a.b" }],
      [{ ...query, bindings: [{ source: "input", key: "q", type: "string", maxLength: 1.5 }] }, { code: "action.max-length-invalid", message: "Invalid maxLength in a.b." }],
      [{ ...query, bindings: [{ source: "input", key: "q", type: "string", nullable: "yes" }] }, { code: "action.binding-nullable-invalid", message: "Action a.b has an invalid nullable input binding." }],
    ];
    for (const [action, editor] of typed) {
      const source = JSON.stringify({ version: 1, actions: { "a.b": action } });
      const kernel = call(checks, "check_actions", source);
      expect(kernel?.code).toBeNull();
      expect(kernel?.message).toStartWith("Invalid actions.json: ");
      expect(outcomeOf(() => parseServerActionSource(source))).toEqual(editor);
    }
    const gate = createCapsuleValidator(() => initSqlJs());
    for (const [schema, refused] of [["CREATE TABLE x AS SELECT 1;", "CREATE TABLE ... AS"], ["CREATE TABLE t (a TEXT CHECK (load_extension('x') IS NULL));", "load_extension()"]]) {
      expect(call(checks, "check_schema", schema)).toBeNull();
      const result = await gate.validate({ ...starterProject.files, "schema.sql": schema, "actions.json": JSON.stringify({ version: 1, actions: {} }) });
      expect(result.errors[0]?.message).toBe(`schema.sql may only create tables and indexes, but it contains ${refused}.`);
    }
  });

  test("answer a file that does not parse without a code, as the Kernel's publish does", () => {
    const { checks } = load();
    expect(call(checks, "check_actions", JSON.stringify({ version: 2, actions: {} }))).toEqual({ code: null, message: "Unsupported actions.json version." });
    expect(call(checks, "check_actions", "{")?.code).toBeNull();
  });
});
