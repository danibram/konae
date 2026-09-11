import { expect, test } from "playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.clear());
});

test("an owner backs up, migrates, and restores an app database from the UI", async ({ page }) => {
  await page.goto("/projects");

  await page.getByRole("button", { name: "New project" }).click();
  await page.getByLabel("Project name").fill("Migration proof");
  await page.getByRole("button", { name: /^Create/ }).click();

  await expect(page.getByRole("button", { name: "Migration proof" })).toBeVisible();
  await page.getByRole("navigation", { name: "Workspace sections" }).getByRole("button", { name: /Data/ }).click();
  await expect(page.getByRole("region", { name: "Database panel" })).toBeVisible();
  await expect(page.getByRole("region", { name: "tasks table editor" })).toBeVisible();

  await page.getByRole("button", { name: "Insert row" }).click();
  await page.locator(".insert-fields label").filter({ hasText: /^title/ }).getByRole("textbox").fill("Keep me");
  await page.locator(".insert-fields label").filter({ hasText: /^created_at/ }).getByRole("textbox").fill("1700000000000");
  await page.locator(".insert-actions").getByRole("button", { name: "Insert row" }).click();

  const dataGrid = page.locator(".editable-grid");
  await expect(dataGrid).toContainText("Keep me");

  await page.getByRole("navigation", { name: "Database views" }).getByRole("button", { name: "Schema" }).click();
  await page.getByRole("button", { name: "Edit schema.ts" }).click();
  await page.getByLabel("schema.ts code editor").fill(`import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const tasks = sqliteTable("tasks", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  title: text("title").notNull(),
  done: integer("done", { mode: "boolean" }).notNull().default(false),
  createdAt: integer("created_at").notNull(),
  priority: text("priority").notNull().default("normal"),
});
`);

  await page.getByRole("navigation", { name: "Workspace sections" }).getByRole("button", { name: /Data/ }).click();
  await page.getByRole("navigation", { name: "Database views" }).getByRole("button", { name: "Schema" }).click();
  await expect(page.getByRole("button", { name: "Generate migration" })).toHaveCount(0);
  await expect(page.getByRole("status")).toContainText("Migration ready · review generated SQL");
  const migrationPlan = page.getByRole("region", { name: "Migration plan" });
  await expect(migrationPlan).toContainText("Migration ready");
  await expect(migrationPlan).toContainText(/ALTER TABLE [`"]tasks[`"] ADD [`"]priority[`"] text DEFAULT 'normal' NOT NULL/i);
  await migrationPlan.getByRole("button", { name: "Apply migration" }).click();
  await expect(page.getByRole("status")).toContainText("Migration applied");

  await page.getByRole("navigation", { name: "Database views" }).getByRole("button", { name: "Data", exact: true }).click();
  await expect(dataGrid.getByRole("columnheader").filter({ hasText: "priority" })).toBeVisible();
  await expect(dataGrid).toContainText("Keep me");
  await expect(dataGrid).toContainText("normal");

  await page.getByRole("navigation", { name: "Database views" }).getByRole("button", { name: "Backups" }).click();
  const backup = page.getByRole("listitem").filter({ hasText: "Before migration" });
  await expect(backup).toBeVisible();
  await backup.getByRole("button", { name: "Restore" }).click();
  await page.getByRole("button", { name: "Confirm restore" }).click();
  await expect(page.getByRole("status")).toContainText("Backup restored");

  await page.getByRole("navigation", { name: "Database views" }).getByRole("button", { name: "Data", exact: true }).click();
  await expect(dataGrid).toContainText("Keep me");
  await expect(dataGrid.getByRole("columnheader").filter({ hasText: "priority" })).toHaveCount(0);

  await page.getByRole("navigation", { name: "Database views" }).getByRole("button", { name: "Schema" }).click();
  await page.getByRole("button", { name: "Edit schema.ts" }).click();
  await expect(page.getByLabel("schema.ts code editor")).not.toContainText("priority");
});
