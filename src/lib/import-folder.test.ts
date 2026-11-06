import { describe, expect, test } from "bun:test";
import { dataEntries, type AppSettings } from "./app-folder";
import { dataManifest, readAppFolder, sourcesPayload } from "./import-folder";
import { checkPackageManifest } from "./portable-package";
import { createTar, readTar } from "./tar";

const settings: AppSettings = {
  id: "notes-1", name: "Notes", slug: "notes", visibility: "private",
  signupPolicy: { mode: "invitation" }, signInMethods: ["password"], kernel: "0.1.0", manifest: { version: 1, routing: "pages" },
};
const bytes = (text: string) => new TextEncoder().encode(text);

/** A folder as the editor's Export writes it: one top folder, data/ from the export's components. */
function exported() {
  const index = JSON.stringify([{ id: "b-file", size: 2 }, { id: "a-file", size: 1 }]);
  const files = createTar([{ path: "index.json", contents: bytes(index) }, { path: "b-file.blob", contents: bytes("bb") }, { path: "a-file.blob", contents: bytes("a") }]);
  const entries = [
    { path: "konae.json", contents: JSON.stringify(settings) },
    { path: "app/actions.json", contents: "{\"version\":1,\"actions\":{}}" },
    { path: "app/schema.sql", contents: "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);" },
    { path: "dist/index.html", contents: "<!doctype html><main>Notes</main>" },
    { path: "dist/logo.png", contents: new Uint8Array([0x89, 0x50, 0xff, 0x00]) },
    ...dataEntries("database", bytes("database bytes")),
    ...dataEntries("identity", bytes("identity bytes")),
    ...dataEntries("files", files),
  ];
  return { archive: createTar(entries.map((entry) => ({ path: `notes/${entry.path}`, contents: entry.contents }))), files };
}

describe("reading an exported app folder", () => {
  test("gives back the sources, the built files and each data component as the export took it", () => {
    const { archive, files } = exported();
    const folder = readAppFolder(archive);
    expect(folder.settings.id).toBe("notes-1");
    expect(Object.keys(folder.sources).sort()).toEqual(["actions.json", "schema.sql"]);
    expect(new TextDecoder().decode(folder.data.database)).toBe("database bytes");
    // The files component comes back in index order, byte for byte.
    expect(readTar(folder.data.files!).map((entry) => entry.path)).toEqual(["index.json", "b-file.blob", "a-file.blob"]);
    expect(folder.data.files).toEqual(files);
    expect(folder.data.usage).toBeUndefined();
  });

  test("makes a release that keeps the app's own ID and visibility, and a manifest the Kernel's rules accept", async () => {
    const folder = readAppFolder(exported().archive);
    const release = sourcesPayload(folder, { id: "target", name: "Target", slug: "target" }, "public");
    expect(release.project).toMatchObject({ id: "target", manifest: { version: 1, routing: "pages", visibility: "public" } });
    expect(release.artifact.files["index.html"]).toBe("<!doctype html><main>Notes</main>");
    expect(release.artifact.encodings).toEqual({ "logo.png": "base64" });
    const manifest = checkPackageManifest(await dataManifest(folder, "target"));
    expect(manifest.contents.map((content) => content.name)).toEqual(["database", "identity", "files"]);
    expect(manifest.projectId).toBe("target");
  });

  test("names what is missing", () => {
    expect(() => readAppFolder(createTar([{ path: "x/readme.md", contents: "no settings" }]))).toThrow("no konae.json");
    const broken = createTar([{ path: "konae.json", contents: JSON.stringify(settings) }, { path: "data/files/index.json", contents: "[{\"id\":\"gone\",\"size\":1}]" }]);
    expect(() => readAppFolder(broken)).toThrow("has no gone.blob");
  });
});
