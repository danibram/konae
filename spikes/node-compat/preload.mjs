import { readFile } from "node:fs/promises";
import { patchLocal } from "./patch.mjs";

Bun.plugin({
  name: "konae-node-compat-spike",
  setup(build) {
    build.onLoad({ filter: /\/src\/lib\/local-builder\.js$/ }, async ({ path }) => ({
      contents: patchLocal(await readFile(path, "utf8"), "../../spikes/node-compat/installed-providers.mjs"),
      loader: "js",
    }));
  },
});
