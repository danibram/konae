import { describe, expect, test } from "bun:test";
import { readFileSync, statSync } from "node:fs";
import path from "node:path";
import { spanishHome } from "./home-site";

const home = path.resolve(import.meta.dir, "../examples/site");

describe("the Konae home", () => {
  test("the Spanish page is a crawlable page with reciprocal language links", () => {
    const english = readFileSync(path.join(home, "index.html"), "utf8");
    const spanish = readFileSync(path.join(home, "pages/es/index.html"), "utf8");
    expect(spanish).toBe(spanishHome(english));
    expect(spanish).toContain('<html lang="es"');
    expect(spanish).toContain("Describe una app.");
    expect(spanish).toContain('hreflang="en" href="../"');
    expect(english).toContain('hreflang="es" href="es/"');
    expect(spanish).not.toContain("data-lang");
  });

  test("each home app card image is at least 300 pixels wide and under 600 KB", () => {
    for (const folder of ["public", "public/docs"]) {
      const target = path.join(home, folder, "card.png");
      const image = readFileSync(target);
      expect(image.byteLength).toBeLessThan(600 * 1024);
      expect(image.readUInt32BE(16)).toBeGreaterThanOrEqual(300);
      expect(image[25]).toBe(2);
      expect(statSync(target).size).toBe(image.byteLength);
      expect(readFileSync(path.join(home, folder, "card.svg"), "utf8")).not.toContain('rx="42"');
    }
  });

  test("the page script redirects old Spanish links and preserves invitation language", () => {
    const script = readFileSync(path.join(home, "home.js"), "utf8");
    expect(script).toContain('new URLSearchParams(location.search).get("lang") === "es"');
    expect(script).toContain('location.replace("/es/")');
    expect(script).toContain('`${links.invite}?lang=${lang}`');
  });
});
