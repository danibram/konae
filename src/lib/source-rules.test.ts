import { describe, expect, test } from "bun:test";
import file from "../../kernel-rs/kernel/src/sources-cases.json";
import { refusalOf } from "../kernel/refusal";
import { artifactRefused, carriedBytes, checkSourceUpload, releaseRefused, sizeText, uploadRefused, UPLOAD_BYTES } from "./size";
import { checkSourceFiles, checkSources } from "./source-rules";

type Case = { case: string; files: Record<string, number>; more?: number; error: string | null; code?: string };

describe("the rules the sources of app/ meet", () => {
  test("the case file covers every rule", () => {
    expect(file.cases.length).toBeGreaterThanOrEqual(20);
  });
  for (const item of file.cases as Case[]) {
    test(item.case, () => {
      const files: [string, number][] = [...Object.entries(item.files), ...Array.from({ length: item.more ?? 0 }, (_, n) => [`more/${n}.txt`, 1] as [string, number])];
      let outcome: unknown = null;
      try { checkSources(files); } catch (error) { outcome = refusalOf(error) ?? error; }
      if (item.error === null) expect(outcome).toBeNull();
      else expect(outcome).toMatchObject({ code: item.code ?? "the case names no code", message: item.error });
    });
  }

  test("a document counts each file's decoded bytes", () => {
    const png = "iVBORw0KGgo=";
    expect(carriedBytes(png, true)).toBe(8);
    expect(carriedBytes("AA==", true)).toBe(1);
    expect(carriedBytes("héllo \u{1F600}", false)).toBe(new TextEncoder().encode("héllo \u{1F600}").length);
    const big = "A".repeat(Math.ceil((20 * 1024 * 1024 + 1) / 3) * 4);
    expect(() => checkSourceFiles({ files: { "index.html": "<main></main>", "public/big.bin": big }, encodings: { "public/big.bin": "base64" } })).toThrow("app/ holds 20.1 MiB. A project's sources hold at most 20 MiB.");
    expect(() => checkSourceFiles({ files: { "index.html": big } })).toThrow("app/ holds 26.7 MiB.");
  });

  test("a serialized source document names the transfer size and limit before upload", () => {
    expect(() => checkSourceUpload(UPLOAD_BYTES + 1)).toThrow(uploadRefused(UPLOAD_BYTES + 1));
  });
});

describe("the size sentences", () => {
  for (const item of file.sizes) test(`${item.bytes} bytes`, () => expect(sizeText(item.bytes)).toBe(item.text));
  for (const item of file.sentences) {
    test(item.case, () => {
      const sentence = item.limit === "artifact" ? artifactRefused(Object.entries(item.files ?? {}))
        : item.limit === "release" ? releaseRefused(item.bytes!)
        : uploadRefused(item.bytes!);
      expect(sentence).toBe(item.sentence);
    });
  }
});
