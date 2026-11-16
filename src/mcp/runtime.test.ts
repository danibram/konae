import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

async function runtime(params: string) {
  const source = await readFile(new URL("./runtime.js", import.meta.url), "utf8");
  const meta = { getAttribute: () => params };
  const document = { querySelector: () => meta };
  const window: Record<string, any> = {};
  new Function("window", "location", "document", source)(window, { pathname: "/p/demo/post" }, document);
  return window.miniapp;
}

test("page URLs reject browser-normalized cross-origin paths", async () => {
  const miniapp = await runtime("{}");
  for (const path of ["/\\evil.example/x", "/\tevil.example/x", "/\nevil.example/x"]) {
    expect(() => miniapp.page.url(path)).toThrow("An app page path must start with one slash.");
  }
  expect(miniapp.page.url("/posts/hello")).toBe("/p/demo/posts/hello");
});

test("page params always return an object", async () => {
  expect(await runtime("null").then((miniapp) => miniapp.page.params)).toEqual({});
  expect(await runtime("[]").then((miniapp) => miniapp.page.params)).toEqual({});
  expect(await runtime('{"slug":"hello"}').then((miniapp) => miniapp.page.params)).toEqual({ slug: "hello" });
});
