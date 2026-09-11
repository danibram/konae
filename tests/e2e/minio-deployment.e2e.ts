import { expect, test } from "playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.clear());
});

test("builds a multi-file app in the browser, deploys it to MinIO, and persists its data", async ({ page }) => {
  const storage = await page.request.post("/_littlebox/admin/storage/test", {
    headers: { "x-littlebox-admin-key": "local-development-key" },
  });
  expect(storage.ok()).toBe(true);
  await expect(storage.json()).resolves.toMatchObject({
    result: { ok: true, adapter: "s3-compatible", bucket: "littlebox-apps" },
  });

  await page.goto("/projects");

  await page.getByRole("button", { name: "New project" }).click();
  await page.getByLabel("Project name").fill("MinIO tasks");
  await page.getByRole("button", { name: /^Create/ }).click();

  const explorer = page.getByRole("navigation", { name: "Project files" });
  await explorer.getByRole("button", { name: "New folder" }).click();
  await page.getByLabel("Folder path").fill("src/features");
  await page.getByRole("button", { name: "Create" }).click();
  await expect(explorer.getByRole("button", { name: "features" })).toBeVisible();

  await explorer.getByRole("button", { name: "New file" }).click();
  await page.getByLabel("File path").fill("src/features/empty-label.js");
  await page.getByRole("button", { name: "Create" }).click();
  await page.getByLabel("src/features/empty-label.js code editor").fill("export const emptyLabel = 'No tasks stored in MinIO yet.';\n");

  await explorer.getByRole("button", { name: "app.js" }).click();
  const appEditor = page.getByLabel("app.js code editor");
  const appSource = await appEditor.inputValue();
  await appEditor.fill(
    `import { emptyLabel } from './src/features/empty-label.js';\n${appSource.replace(
      "'<div class=\"empty\">Nothing here. Lovely.</div>'",
      "'<div class=\"empty\">' + emptyLabel + '</div>'",
    )}`,
  );

  await page.getByRole("button", { name: "Run", exact: true }).click();
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(preview.getByText("No tasks stored in MinIO yet.")).toBeVisible();

  await page.getByRole("button", { name: "Deploy", exact: true }).click();
  await page.getByRole("button", { name: "Deploy public" }).click();
  const publishedUrl = page.locator("a.published-url");
  await expect(publishedUrl).toBeVisible({ timeout: 30_000 });
  const deploymentUrl = await publishedUrl.getAttribute("href");
  expect(deploymentUrl).toMatch(/^http:\/\/minio-tasks-[a-f0-9]{6}\.localhost:8080$/);

  await page.goto(deploymentUrl!);
  await expect(page.getByText("No tasks stored in MinIO yet.")).toBeVisible();
  await page.getByPlaceholder(/Add one small thing/).fill("Ship the S3 build");
  await page.getByRole("button", { name: "Add task" }).click();
  await expect(page.getByText("Ship the S3 build")).toBeVisible();

  await page.reload();
  await expect(page.getByText("Ship the S3 build")).toBeVisible();
  await page.getByRole("button", { name: "Toggle task" }).click();
  await expect(page.locator("article.task.done")).toContainText("Ship the S3 build");
});
