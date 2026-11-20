# Exported custom build project

This fixture checks that an app folder can rebuild its editable source after export and re-export without Git or dependencies inherited from the Konae checkout.

`app/build.json` declares build inputs. Each declared file lives under `app/build/<path>` and projects to `<path>` at the app folder root. The manifest travels with `app/`, so import and re-export use the same declaration. Export, import, validation and publish read or copy these files but do not run their custom build script. The owner runs `npm ci` and `npm run build` locally, or runs `konae build` explicitly.

The fixture uses synthetic React, TypeScript and Hono files. It covers `@repo/*` and `@/*` aliases, `tsconfig` extension, a PNG shim, the exported package locks, a Functions bundle, credential/data/cache exclusions, and keeping a published `dist/` and `server/` beside newer draft sources. It deletes generated `dist/` and `server/` before invoking the current CLI builder.

Run from the repository checkout:

```sh
bun run spikes/functions-runtime/evidence/2026-11-20-export-build/verify-export-build.mjs
```

The script creates and removes a temporary fixture. It writes the command results to `results.json`. The last run passed all six checks. The focused platform suite passed 23 tests with 92 assertions.

For an app that declares this contract, an exported folder can be rebuilt with the exported root and app lockfiles:

```sh
cd /path/to/export
npm ci
(cd app && npm ci)
npm run build
bun run /path/to/miniapp-builder/scripts/konae.ts build --dir .
```

The fixture does not inspect or export a real app's data. It makes no production connection and does not run custom build code during import or export.
