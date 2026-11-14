import { describe, expect, test } from "bun:test";
import { encodeBase64 } from "../mcp/client";
import { isEncoded, keptEncodings, sourceBytes, sourceFilesFrom, textFiles, utf8Text } from "./source-files";

const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0, 0xff]);
const encoder = new TextEncoder();

describe("source files", () => {
  test("a file is text when its bytes are UTF-8, a byte order mark included, and base64 otherwise", () => {
    const withBom = new Uint8Array([0xef, 0xbb, 0xbf, ...encoder.encode("{}")]);
    expect(utf8Text(withBom)).toBe("﻿{}");
    expect(utf8Text(png)).toBeNull();
    const source = sourceFilesFrom([["index.html", encoder.encode("<main></main>")], ["public/logo.png", png], ["site.webmanifest", withBom]]);
    expect(source).toEqual({ files: { "index.html": "<main></main>", "public/logo.png": encodeBase64(png), "site.webmanifest": "﻿{}" }, encodings: { "public/logo.png": "base64" } });
    expect(sourceBytes(source, "public/logo.png")).toEqual(png);
    expect(sourceBytes(source, "site.webmanifest")).toEqual(withBom);
    expect(textFiles(source)).toEqual({ "index.html": "<main></main>", "site.webmanifest": "﻿{}" });
  });

  test("sources that are all text have no encodings", () => {
    expect(sourceFilesFrom([["index.html", encoder.encode("<main></main>")]])).toEqual({ files: { "index.html": "<main></main>" } });
  });

  test("an encoding follows its file through a move, and goes with a delete or a text write", () => {
    const encodings = { "public/a.png": "base64", "public/b.png": "base64" } as const;
    expect(keptEncodings(encodings, { "img/a.png": "", "public/b.png": "" }, { rename: (path) => path.replace(/^public\/a/, "img/a") })).toEqual({ "img/a.png": "base64", "public/b.png": "base64" });
    expect(keptEncodings(encodings, { "public/b.png": "" })).toEqual({ "public/b.png": "base64" });
    expect(keptEncodings(encodings, { "public/b.png": "" }, { replaced: ["public/b.png"] })).toBeUndefined();
    expect(isEncoded({ files: {}, encodings }, "public/a.png")).toBe(false);
  });
});
