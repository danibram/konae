import { readFile } from "node:fs/promises";
import { dirname, sep } from "node:path";
import { transform } from "esbuild";

export const npmMode = {
  name: "konae-spike-npm-mode",
  setup(build) {
    build.onLoad({ filter: /\.[cm]?js$/ }, async ({ path }) => {
      if (!path.includes(`${sep}node_modules${sep}`)) return;
      const source = await readFile(path, "utf8");
      const contents = source.includes("NODE_ENV")
        ? (await transform(source, {
          loader: "js",
          sourcefile: path,
          define: {
            "process.env.NODE_ENV": '"production"',
            "global.process.env.NODE_ENV": '"production"',
          },
        })).code
        : source;
      return { contents, resolveDir: dirname(path), loader: "js" };
    });
  },
};
