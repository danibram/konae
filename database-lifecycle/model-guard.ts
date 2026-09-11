import path from "node:path";
import { fileURLToPath } from "node:url";

const maximumModelBytes = 50_000;
const allowedImport = "drizzle-orm/sqlite-core";

/** The repository root, so module resolution never depends on the Kernel's working directory. */
export function repoRoot(): string {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
}

/**
 * Rejects a Database Model that is empty, oversized, or reaches past the one
 * import a Managed Project allows.
 *
 * This is a static guard over source text, so it is defeatable by construction —
 * `Function("return pro" + "cess")()` reads nothing this regular expression can
 * see. It exists to fail fast with a clear message, not to contain anything.
 * Containment is the separate process the evaluation runs in (ADR-0007).
 */
export function assertConstrainedModel(source: string, label: string): void {
  if (!source.trim()) throw new Error(`${label} is empty.`);
  if (Buffer.byteLength(source) > maximumModelBytes) throw new Error(`${label} is larger than 50 KB.`);

  const imports = [...source.matchAll(/\b(?:import|export)\s+[\s\S]*?\sfrom\s+["']([^"']+)["']/g)].map((match) => match[1]);
  if (imports.length === 0 || imports.some((specifier) => specifier !== allowedImport)) {
    throw new Error(`${label} may only import from ${allowedImport}.`);
  }
  if (/\b(?:import\s*\(|require\s*\(|process\b|globalThis\b|fetch\s*\(|WebSocket\b|XMLHttpRequest\b|Bun\b|Deno\b|node:)/.test(source)) {
    throw new Error(`${label} contains executable capabilities that Managed Projects do not allow.`);
  }
}
