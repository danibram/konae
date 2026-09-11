import architectureSource from "../../public/architecture.html?raw";
import { createDefaultManifest } from "../deployments/manifest";
import type { MiniAppProject } from "./types";

export const architectureProjectId = "app_littlebox_architecture";
// Seed copy is public/architecture.html. A stored project refreshes while it still names the old map or the simulator label.

const architectureFiles = splitArchitectureSource(architectureSource);

export function cloneArchitectureProject(): MiniAppProject {
  const manifest = createDefaultManifest();
  manifest.runtime.database.actions = [];

  return {
    id: architectureProjectId,
    name: "Littlebox Architecture",
    slug: "architecture",
    manifest,
    updatedAt: Date.parse("2026-08-14T00:00:00Z"),
    databaseModel: {
      source: `import { sqliteTable } from "drizzle-orm/sqlite-core";\n\n// This project has no database tables.\nvoid sqliteTable;\n`,
      acceptedSource: `import { sqliteTable } from "drizzle-orm/sqlite-core";\n\n// This project has no database tables.\nvoid sqliteTable;\n`,
      revision: "baseline",
    },
    directories: [],
    files: {
      ...architectureFiles,
      "schema.sql": "",
      "actions.json": JSON.stringify({ version: 1, actions: {} }, null, 2),
    },
  };
}

function splitArchitectureSource(source: string): Pick<MiniAppProject["files"], "index.html" | "styles.css" | "app.js"> {
  const styles = source.match(/<style>([\s\S]*?)<\/style>/i)?.[1]?.trim();
  const body = source.match(/<body>([\s\S]*?)<script>/i)?.[1]?.trim();
  const script = source.match(/<script>([\s\S]*?)<\/script>\s*<\/body>/i)?.[1]?.trim();

  if (styles === undefined || body === undefined || script === undefined) {
    throw new Error("public/architecture.html must contain one style block, body and script block.");
  }

  return {
    "index.html": body,
    "styles.css": styles,
    "app.js": script,
  };
}
