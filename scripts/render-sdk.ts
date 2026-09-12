// Writes the three readers of the SDK contract: the build agent's prompt
// section, the platform_guide sdk topic and docs/sdk.md.
// src/sdk/contract.test.ts fails when a checked-in file differs from this output.
import { writeFile } from "node:fs/promises";
import path from "node:path";
import { renderAll } from "../src/sdk/render";

const root = path.resolve(import.meta.dir, "..");
for (const [file, contents] of Object.entries(renderAll())) {
  await writeFile(path.join(root, file), contents);
  console.log(`wrote ${file}`);
}
