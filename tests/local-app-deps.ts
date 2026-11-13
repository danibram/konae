import { readFile, symlink } from "node:fs/promises";
import path from "node:path";

const repo = path.resolve(import.meta.dir, "..");

/** Links the repository's pinned esbuild into an initialized app without installing packages. */
export async function provideLocalAppDependencies(folder: string) {
  // Ignore machine npm settings so build output stays repeatable.
  process.env.NPM_CONFIG_USERCONFIG = "/dev/null";
  const manifest = JSON.parse(await readFile(path.join(folder, "package.json"), "utf8")) as { devDependencies?: Record<string, string> };
  const expected = manifest.devDependencies?.esbuild;
  if (!expected) throw Error("The initialized app does not declare esbuild.");

  const modules = path.join(repo, "node_modules");
  const installed = JSON.parse(await readFile(path.join(modules, "esbuild/package.json"), "utf8")) as { version?: string };
  if (installed.version !== expected) {
    throw Error(`The initialized app needs esbuild ${expected}, but the repository has ${installed.version ?? "no version"}.`);
  }
  await symlink(modules, path.join(folder, "node_modules"), "dir");
}
