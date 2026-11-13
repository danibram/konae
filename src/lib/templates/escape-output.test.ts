import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { cloneStarter } from "../starter";
import { applyTemplate, templateChoices, type TemplateId } from "./index";

const born = (id: TemplateId) => applyTemplate({ ...cloneStarter(), id: "app_escape_test", name: "Test", slug: "test" }, id);

/** The escapeHtml a template's app.js defines, compiled on its own. */
function templateEscape(source: string): (value: unknown) => string {
  const match = source.match(/function escapeHtml\(value\) \{[\s\S]*?\n\}/);
  if (!match) throw new Error("app.js defines no escapeHtml");
  return new Function(`${match[0]}\nreturn escapeHtml;`)() as (value: unknown) => string;
}

describe("a template escapes text it writes into HTML", () => {
  for (const { id } of templateChoices) {
    test(`${id} escapes ampersands, angle brackets and both quotes, and writes nothing for null`, () => {
      const escapeHtml = templateEscape(born(id).files["app.js"] as string);
      expect(escapeHtml(`a"b'c<d>&e`)).toBe("a&quot;b&#39;c&lt;d&gt;&amp;e");
      expect(escapeHtml(null)).toBe("");
      expect(escapeHtml(undefined)).toBe("");
      expect(escapeHtml(12)).toBe("12");
    });

    test(`${id} keeps no file that reads a text node's innerHTML`, () => {
      for (const [name, source] of Object.entries(born(id).files)) {
        expect(source, `${id}/${name}`).not.toMatch(/return\s+\w+\.innerHTML/);
      }
    });
  }

  test("the rendered templates file keeps none of them either", () => {
    const rendered = readFileSync(path.join(import.meta.dir, "../../../kernel-rs/konae/src/project-templates.json"), "utf8");
    expect(rendered).not.toMatch(/return\s+\w+\.innerHTML/);
  });

  test("a photo title that closes its attribute stays text in the gallery", async () => {
    const hostile = `x" style="position:fixed;inset:0;background:#fff" onload="alert(1)" data-x="`;
    const element = () => ({ hidden: false, textContent: "", innerHTML: "", files: [], dataset: {}, addEventListener() {} });
    const elements = new Map<string, ReturnType<typeof element>>();
    const document = { querySelector: (selector: string) => elements.get(selector) ?? elements.set(selector, element()).get(selector)! };
    const miniapp = {
      auth: { session: async () => ({ role: "member", name: "Ana" }), signIn: async () => {} },
      actions: {
        call: async (name: string) => name === "photos.list" ? [{ file_id: `f"1`, title: hostile }] : [{ file_id: `f"1` }],
      },
      files: { url: async () => `blob:x" onerror="alert(1)` },
    };
    const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor as new (...args: string[]) => (...values: unknown[]) => Promise<void>;
    await new AsyncFunction("document", "miniapp", born("gallery").files["app.js"] as string)(document, miniapp);
    const html = elements.get("#photos")!.innerHTML;
    // Every quote a value carried is an entity, so no attribute can start from a title, a file id or a URL.
    expect(html).not.toContain(`" style=`);
    expect(html).not.toContain(`" onload=`);
    expect(html).not.toContain(`" onerror=`);
    expect(html).not.toContain(`data-remove="f"1`);
    expect(html).toContain(`alt="x&quot; style=&quot;position:fixed`);
    expect(html).toContain(`src="blob:x&quot; onerror=&quot;alert(1)"`);
  });

  test("a member's role and id stay text in the members list", async () => {
    const element = () => ({ hidden: false, textContent: "", innerHTML: "", value: "", dataset: {}, addEventListener() {} });
    const elements = new Map<string, ReturnType<typeof element>>();
    const document = { querySelector: (selector: string) => elements.get(selector) ?? elements.set(selector, element()).get(selector)!, addEventListener() {} };
    const rows: Record<string, unknown[]> = {
      "me.join": [],
      "me.get": [{ role: "owner", user_id: "me" }],
      "posts.list": [],
      "members.list": [{ display_name: "Ana", role: `member"><img src=x onerror=alert(1)>`, user_id: `u"1` }, { display_name: "Ben", role: "member", user_id: `u"2` }],
    };
    const miniapp = {
      auth: { session: async () => ({ role: "member", name: "me@example.com" }), signIn: async () => {}, signOut: async () => {} },
      actions: { call: async (name: string) => rows[name] ?? [] },
    };
    const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor as new (...args: string[]) => (...values: unknown[]) => Promise<void>;
    await new AsyncFunction("document", "miniapp", born("members").files["app.js"] as string)(document, miniapp);
    const html = elements.get("#members")!.innerHTML;
    expect(html).not.toContain("<img");
    expect(html).toContain("member&quot;&gt;&lt;img src=x onerror=alert(1)&gt;");
    expect(html).not.toContain(`data-promote="u"`);
  });
});
