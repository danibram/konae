import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { cloneStarter } from "../starter";
import { renderAppUi } from "../../../scripts/app-ui-render";
import { appUiStyles } from "./app-ui.generated";
import { applyTemplate, templateChoices } from "./index";

describe("the design system a published app carries", () => {
  test("the checked-in stylesheet is what the renderer writes (run `bun run ui:render`)", () => {
    expect(readFileSync(path.resolve(import.meta.dir, "app-ui.generated.ts"), "utf8")).toBe(renderAppUi());
  });

  test("each variant is self-contained and keeps the editor's chrome out", () => {
    for (const css of Object.values(appUiStyles)) {
      expect(css).toContain("--ds-accent");
      expect(css).toContain(".ui-list-item");
      expect(css).not.toContain(".account-sidebar");
      expect(css).not.toContain("[data-ds=");
      expect(css).not.toContain(".ui-dialog");
    }
    expect(appUiStyles.clean).toContain("--ds-bw: 1px");
    expect(appUiStyles.brutal).not.toContain("--ds-bw: 1px");
    expect(appUiStyles.clean.length).toBeLessThan(40_000);
  });

  test("every template ships the design system and speaks its classes", () => {
    for (const { id } of templateChoices) {
      const project = applyTemplate({ ...cloneStarter(), id: "app_ds_test", name: "Test", slug: "test" }, id);
      const html = project.files["index.html"]!;
      expect(project.files["styles.css"]).toContain(appUiStyles.clean);
      expect(html).toContain("ui-");
      expect(html).not.toMatch(/class="(app-shell|notes-shell|card|primary|quiet|rows|section-label|eyebrow|intro)"/);
    }
  });
});
