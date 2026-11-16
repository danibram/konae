import { describe, expect, test } from "bun:test";
import { sourceDigest } from "./source-digest";

type Case = { files: Record<string, string>; encodings?: Record<string, "base64">; digest: string };
const file: { cases: Case[] } = await Bun.file(new URL("../../kernel-rs/kernel/src/source-digest-cases.json", import.meta.url)).json();

describe("source digest", () => {
  test("every case in kernel-rs/kernel/src/source-digest-cases.json gets the Kernel's digest", async () => {
    expect(file.cases.length).toBeGreaterThan(0);
    expect(file.cases.some((item) => item.encodings)).toBe(true);
    for (const { files, encodings, digest } of file.cases) expect(await sourceDigest({ files, encodings })).toBe(digest);
  });

  test("the order files arrive in does not matter", async () => {
    const [first] = file.cases;
    const reversed = Object.fromEntries(Object.entries(first!.files).reverse());
    expect(await sourceDigest({ files: reversed })).toBe(first!.digest);
  });
});
