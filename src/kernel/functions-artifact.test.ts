import { expect, test } from "bun:test";
import cases from "../../kernel-rs/kernel/src/functions-artifact-cases.json";
import { parseFunctions, validateServerArtifact, serverFolderEntries, serverArtifactFromFolder, verifyServerFolder } from "./functions";

for (const fixture of cases) test(`server artifact parity: ${fixture.name}`, async () => {
  const source = fixture.sourceFromArtifact ? JSON.stringify(fixture.artifact.manifest) : fixture.source;
  const bytes = fixture.filledBundleBytes;
  const input = bytes && fixture.artifact?.bundles ? {
    ...fixture.artifact,
    bundles: Object.fromEntries(Object.entries(fixture.artifact.bundles).map(([name, bundle]) =>
      [name, `${bundle}/*${"x".repeat(bytes - bundle.length - 4)}*/`]
    )),
  } : fixture.artifact;
  const run = () => validateServerArtifact(source === null ? undefined : parseFunctions(source), input);
  if (fixture.ok) {
    const artifact = run();
    expect(artifact ?? null).toEqual(fixture.artifact);
    if (fixture.metadataBytes && artifact) {
      const entries = await serverFolderEntries(artifact);
      const files = Object.fromEntries(entries.map(({ path, contents }) => [path.slice(7), new TextEncoder().encode(contents)]));
      expect(files["manifest.json"].length).toBe(fixture.metadataBytes);
      expect(serverArtifactFromFolder(files)).toEqual(artifact);
      await verifyServerFolder(files);
    }
  } else {
    try { run(); throw Error("Expected a refusal"); }
    catch (error) { expect(error).toMatchObject({ code: fixture.code, message: fixture.message }); }
  }
});

test("portable reader refuses raw manifest padding beyond 256 KiB", async () => {
  const fixture = cases.find((item) => item.metadataBytes === 262144);
  if (!fixture) throw Error("Missing exact metadata boundary fixture.");
  const artifact = validateServerArtifact(parseFunctions(JSON.stringify(fixture.artifact.manifest)), fixture.artifact);
  if (!artifact) throw Error("Missing boundary artifact.");
  const entries = await serverFolderEntries(artifact);
  const files = Object.fromEntries(entries.map(({ path, contents }) => [path.slice(7), new TextEncoder().encode(contents)]));
  const padded = new Uint8Array(262145);
  padded.set(files["manifest.json"]);
  padded[padded.length - 1] = 0x20;
  expect(() => serverArtifactFromFolder({ ...files, "manifest.json": padded })).toThrow("Invalid server manifest file.");
});
