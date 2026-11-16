import { describe, expect, test } from "bun:test";
import { readFileSync, statSync } from "node:fs";
import path from "node:path";
import { spanishHome } from "./home-site";

const home = path.resolve(import.meta.dir, "../examples/home");

describe("the Konae home", () => {
  test("the Spanish page is a crawlable page with reciprocal language links", () => {
    const english = readFileSync(path.join(home, "index.html"), "utf8");
    const spanish = readFileSync(path.join(home, "pages/es/index.html"), "utf8");
    expect(spanish).toBe(spanishHome(english));
    expect(spanish).toContain('<html lang="es">');
    expect(spanish).toContain("Describe una app.");
    expect(spanish).toContain('hreflang="en" href="../"');
    expect(english).toContain('hreflang="es" href="es/"');
    expect(spanish).not.toContain("data-lang");
  });

  test("each home app card image is at least 300 pixels wide and under 600 KB", () => {
    for (const app of ["home", "docs-site"]) {
      const target = path.resolve(home, `../${app}/public/card.png`);
      const image = readFileSync(target);
      expect(image.byteLength).toBeLessThan(600 * 1024);
      expect(image.readUInt32BE(16)).toBeGreaterThanOrEqual(300);
      expect(statSync(target).size).toBe(image.byteLength);
    }
  });
});
