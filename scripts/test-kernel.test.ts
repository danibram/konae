import { afterEach, expect, test } from "bun:test";
import { chmod, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { requireTestKernel, TEST_KERNEL_REQUIRED } from "./test-kernel";

const folders: string[] = [];
afterEach(async () => { for (const folder of folders.splice(0)) await rm(folder, { recursive: true, force: true }); });

async function executable(code: string) {
  const root = await mkdtemp(path.join(tmpdir(), "test-kernel-probe-"));
  folders.push(root);
  const binary = path.join(root, "kernel");
  await writeFile(binary, `#!${process.execPath}\n${code}\n`);
  await chmod(binary, 0o700);
  return { root, binary };
}

test("a compatible Kernel is probed without access to the app's data directory", async () => {
  const { root, binary } = await executable(`
    const {realpathSync} = await import('node:fs');
    await Bun.write(new URL('probe.json', import.meta.url), JSON.stringify({ args: process.argv.slice(2), data: process.env.KONAE_DATA_DIR, realData: realpathSync(process.env.KONAE_DATA_DIR), bind: process.env.KONAE_BIND, cwd: process.cwd(), secret: process.env.KONAE_MAIL_SECRET }));
    console.log(JSON.stringify({executionTest: 1}));
  `);
  const oldData = process.env.KONAE_DATA_DIR;
  const oldSecret = process.env.KONAE_MAIL_SECRET;
  process.env.KONAE_DATA_DIR = root;
  process.env.KONAE_MAIL_SECRET = "not-for-the-probe";
  try { await requireTestKernel(binary); }
  finally {
    if (oldData === undefined) delete process.env.KONAE_DATA_DIR; else process.env.KONAE_DATA_DIR = oldData;
    if (oldSecret === undefined) delete process.env.KONAE_MAIL_SECRET; else process.env.KONAE_MAIL_SECRET = oldSecret;
  }
  const observed = JSON.parse(await readFile(path.join(root, "probe.json"), "utf8"));
  expect(observed.args).toEqual(["capabilities"]);
  expect(observed.data).not.toBe(root);
  expect(observed.cwd).toBe(observed.realData);
  expect(observed.bind).toBe("127.0.0.1");
  expect(observed.secret).toBeUndefined();
  await expect(stat(observed.data)).rejects.toThrow();
});

test.each([
  'console.log("legacy startup message")',
  'console.log(JSON.stringify({executionTest: 2}))',
  'console.log(JSON.stringify({executionTest: 1})); process.exit(1)',
  'console.log(JSON.stringify({}))',
])("an incompatible capability response refuses the local run: %s", async (code) => {
  const { binary } = await executable(code);
  await expect(requireTestKernel(binary)).rejects.toThrow(TEST_KERNEL_REQUIRED);
});

test("a legacy binary that starts serving is stopped and refused", async () => {
  const { binary } = await executable("setInterval(() => {}, 1000)");
  await expect(requireTestKernel(binary, 100)).rejects.toThrow(TEST_KERNEL_REQUIRED);
});
