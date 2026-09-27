import { describe, expect, test } from "bun:test";
import { safeReturnPath } from "./return-path";

const origin = "https://editor.example";

describe("safeReturnPath", () => {
  test("keeps a path on the editor's origin", () => {
    expect(safeReturnPath("/app/p1?tab=data#rows", origin)).toBe("/app/p1?tab=data#rows");
    expect(safeReturnPath("/oauth/authorize?client_id=a", origin)).toBe("/oauth/authorize?client_id=a");
  });

  test("refuses every form the browser resolves to another site", () => {
    for (const raw of [
      "//evil.example", "/\\evil.example", "/\t/evil.example", "/\n/evil.example", "/\r/evil.example",
      "/\t\\evil.example", "\t//evil.example", "https://evil.example", "javascript:alert(1)", "evil.example", "",
    ]) expect(safeReturnPath(raw, origin)).toBeNull();
    expect(safeReturnPath(null, origin)).toBeNull();
  });
});
