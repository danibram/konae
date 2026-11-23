import { writeFile } from "node:fs/promises";
import { bundleProviders } from "../src/lib/node-compat-provider-sources.mjs";

const providers = await bundleProviders();
await writeFile(new URL("../src/lib/node-compat-providers.js", import.meta.url), `export const providers = new Map(${JSON.stringify([...providers])});\n`);
