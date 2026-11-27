import { expect, test } from "bun:test";
import cases from "../../kernel-rs/kernel/src/functions-cases.json";
import cronCases from "../../kernel-rs/kernel/src/cron-cases.json";
import { parseFunctions, parseFunctionSources, validateFunctions, validateServerArtifact, serverFolderEntries, serverArtifactFromFolder, verifyServerFolder, type ServerArtifact } from "./functions";

const cronFunctions = JSON.stringify({ version: 1, functions: {
  digest: { entry: "functions/digest.ts", access: "scheduled" },
  web: { entry: "functions/web.ts", access: "anonymous", mount: "/web", methods: ["POST"] },
} });

for (const fixture of cronCases) test(`cron parity: ${fixture.case}`, () => {
  const run = () => parseFunctionSources(cronFunctions, JSON.stringify(fixture.crons));
  if ("expression" in fixture) expect(run()?.crons?.digest.cron).toBe(fixture.expression);
  else {
    try { run(); throw Error("Expected a refusal"); }
    catch (error) { expect(error).toMatchObject({ code: fixture.code, message: fixture.error }); }
  }
});

test("rebuilding is required when crons.json changes", () => {
  const source = { version: 1, crons: { digest: { cron: "0 8 * * *", function: "digest" } } };
  const manifest = parseFunctionSources(cronFunctions, JSON.stringify(source));
  const artifact = {
    version: 1, abi: "konae-functions/1", manifest,
    bundles: { digest: "export function handle() {}", web: "export function handle() {}" },
  };
  source.crons.digest.cron = "0 9 * * *";
  const changed = parseFunctionSources(cronFunctions, JSON.stringify(source));
  expect(() => validateServerArtifact(changed, artifact)).toThrow("Server Artifact manifest does not match crons.json.");
});

for (const fixture of cases) test(`functions parity: ${fixture.name}`, () => {
  const run = () => { const manifest = parseFunctions(fixture.source); validateFunctions(manifest, fixture); return manifest; };
  if ("expected" in fixture) expect(run()).toEqual(fixture.expected);
  else {
    try { run(); throw Error("Expected a refusal"); }
    catch (error) { expect(error).toMatchObject({ code: fixture.code, message: fixture.message }); }
  }
});

const manifest = parseFunctions(JSON.stringify({ version: 1, functions: { orders: { entry: "functions/orders.ts", mount: "/api/orders", methods: ["GET"], access: "anonymous" } } }));
const artifact: ServerArtifact = { version: 1, abi: "konae-functions/1", manifest, bundles: { orders: 'throw Error("must never execute"); export function handle() {}' } };

test("artifact validation checks bytes without evaluating top-level code", () => {
  const prefix = "export const handle=()=>null;";
  const fullBundle = `${prefix}/*${"x".repeat(1048576 - prefix.length - 4)}*/`;
  expect(validateServerArtifact(manifest, artifact)).toEqual(artifact);
  expect(() => validateServerArtifact(manifest, undefined)).toThrow("Declared functions require a Server Artifact.");
  expect(() => validateServerArtifact(manifest, { ...artifact, abi: "other" })).toThrow("Server Artifact requires version 1 and ABI konae-functions/1.");
  expect(() => validateServerArtifact(manifest, { ...artifact, bundles: {} })).toThrow("Server Artifact bundles must match the declared function names.");
  expect(() => validateServerArtifact(manifest, { ...artifact, bundles: { orders: "é".repeat(524289) } })).toThrow("1048576 UTF-8 bytes");
  expect(validateServerArtifact(manifest, { ...artifact, bundles: { orders: fullBundle } })).toBeDefined();
});

test("artifact AST validation walks deeply nested expressions iteratively", () => {
  const bundle = `export const handle=()=>${"1+".repeat(12000)}1;`;
  expect(validateServerArtifact(manifest, { ...artifact, bundles: { orders: bundle } })?.bundles.orders).toBe(bundle);
});

test("server folder verifies hashes and does not accept extra files", async () => {
  const entries = await serverFolderEntries(artifact);
  const files = Object.fromEntries(entries.map(({ path, contents }) => [path.slice(7), new TextEncoder().encode(contents)]));
  expect(serverArtifactFromFolder(files)).toEqual(artifact);
  await verifyServerFolder(files);
  await expect(verifyServerFolder({ ...files, "orders.mjs": new TextEncoder().encode("export const handle = () => null;") })).rejects.toThrow("hashes do not match");
  expect(() => serverArtifactFromFolder({ ...files, "extra.mjs": new Uint8Array() })).toThrow("undeclared file");
});
