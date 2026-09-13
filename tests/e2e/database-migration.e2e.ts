import { expect, test } from "playwright/test";

test.beforeEach(async ({ page }) => {
  // The Kernel limits owner signups per client address. The test server trusts the local
  // proxy, so give each test its own forwarded address and repeated runs stay under the limit.
  const client = `198.51.100.${1 + Math.floor(Math.random() * 254)}`;
  const response = await page.request.post("/_littlebox/owner/signup", { headers: { "x-forwarded-for": client }, data: { email: `owner-${crypto.randomUUID()}@example.test`, password: "Owner-test-password-2026", setupKey: "local-development-key" } });
  expect(response.ok()).toBe(true);
  await page.addInitScript(() => localStorage.clear());
});

test("an owner backs up, migrates, and restores an app database from the UI", async ({ page }) => {
  await page.goto("/projects");

  await page.getByRole("button", { name: "New project" }).click();
  await page.getByLabel("Project name").fill("Migration proof");
  await page.getByRole("button", { name: /^Create/ }).click();

  await expect(page).toHaveURL(/\/app\/[^/]+$/);
  await expect(page.getByRole("banner").locator("summary").getByText("Migration proof", { exact: true })).toBeVisible();
  await page.getByRole("navigation", { name: "Workspace sections" }).getByRole("button", { name: /Data/ }).click();
  await expect(page.getByRole("region", { name: "Database panel" })).toBeVisible();
  await expect(page.getByRole("region", { name: "tasks table editor" })).toBeVisible();

  await page.getByRole("button", { name: "Insert row" }).click();
  await page.getByRole("textbox", { name: /^Title\b/ }).fill("Keep me");
  await page.getByRole("textbox", { name: /^Created\b/ }).fill("1700000000000");
  await page.locator(".insert-actions").getByRole("button", { name: "Insert row" }).click();

  const dataGrid = page.locator(".editable-grid");
  await expect(dataGrid).toContainText("Keep me");

  await page.getByRole("navigation", { name: "Database views" }).getByRole("button", { name: "Schema" }).click();
  await page.getByRole("button", { name: "Edit the columns" }).click();
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
  await page.getByRole("button", { name: "Edit the columns" }).click();
  await expect(page.getByLabel("schema.ts code editor")).not.toContainText("priority");
});
