import { expect, test } from "bun:test";
import cases from "../../kernel-rs/kernel/src/functions-cases.json";
import { parseFunctions, validateFunctions, validateServerArtifact, serverFolderEntries, serverArtifactFromFolder, verifyServerFolder, type ServerArtifact } from "./functions";

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
