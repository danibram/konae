import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cloneStarter } from "../lib/starter";
import { MiniAppRuntimeHost } from "./host";
import { previewDocumentTracker } from "./preview-document";

const previousLocalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");

beforeEach(() => {
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
      removeItem: (key: string) => { values.delete(key); },
    },
  });
});

afterEach(() => {
  if (previousLocalStorage) Object.defineProperty(globalThis, "localStorage", previousLocalStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

const starter = cloneStarter();

describe("preview document tracker", () => {
  test("a second connect for the same document keeps its failures; a new document clears them", async () => {
    const host = new MiniAppRuntimeHost("tracker", starter.files["schema.sql"], starter.files["actions.json"]);
    await host.exportDatabase();
    const documentStarted = previewDocumentTracker(host);
    const first = "<html><script>throw new Error('boom')</script></html>";

    expect(documentStarted(first)).toBe(true);
    await host.handle("preview.error", { kind: "error", message: "boom", stack: "" });
    expect(host.previewErrors().map((failure) => failure.message)).toEqual(["boom"]);

    expect(documentStarted(first)).toBe(false);
    expect(host.previewErrors().map((failure) => failure.message)).toEqual(["boom"]);

    expect(documentStarted("<html><script>/* fixed */</script></html>")).toBe(true);
    expect(host.previewErrors()).toEqual([]);
  });

  test("the first connect of a fresh bridge always clears", () => {
    let clears = 0;
    const documentStarted = previewDocumentTracker({ clearPreviewErrors: () => { clears += 1; } });
    documentStarted("");
    documentStarted("");
    expect(clears).toBe(1);
  });
});
