import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

export const TEST_KERNEL_REQUIRED = "This Kernel does not support isolated local testing. Update the Kernel or set KONAE_KERNEL_BIN to a compatible binary before running dev. Local data was not imported.";

/** Probes an empty directory because older binaries may start a server for an unknown command. */
export async function requireTestKernel(binary: string, timeoutMs = 5000): Promise<void> {
  const directory = await mkdtemp(path.join(tmpdir(), "konae-capabilities-"));
  try {
    const env = {
      ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KONAE_") && !name.startsWith("VITE_"))),
      KONAE_DATA_DIR: directory,
      KONAE_BIND: "127.0.0.1",
      KONAE_KERNEL_PORT: "0",
      KONAE_PUBLIC_ORIGIN: "http://localhost",
      KONAE_HOST_SUFFIX: "localhost",
      KONAE_MAIL_TRANSPORT: "development",
    };
    const child = Bun.spawn([binary, "capabilities"], { cwd: directory, env, stdout: "pipe", stderr: "ignore" });
    const timer = setTimeout(() => child.kill(), timeoutMs);
    try {
      const [code, output] = await Promise.all([child.exited, new Response(child.stdout).text()]);
      let capabilities: unknown;
      try { capabilities = JSON.parse(output); } catch { throw Error(TEST_KERNEL_REQUIRED); }
      if (code !== 0 || !capabilities || typeof capabilities !== "object" || !("executionTest" in capabilities) || capabilities.executionTest !== 1) {
        throw Error(TEST_KERNEL_REQUIRED);
      }
    } finally {
      clearTimeout(timer);
      if (child.exitCode === null) { child.kill(); await child.exited; }
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
