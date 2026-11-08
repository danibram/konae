# dist/ is the publishing contract, and the editor builds in the browser

Status: accepted.

An app may be built in the editor's tab, by `konae build` on a laptop, or by any builder its owner prefers. The Kernel holds every app's database and accounts in one process, so it must never run an app's code or its packages to build it, and it must not trust what a builder hands it.

## Decision

### Any builder may write `dist/`, and the Kernel checks it

The publishing contract is the static files in `dist/` with a manifest of their paths, hashes, sizes, content types, `encodings` and `routing`. It is not a compiler or a project layout. The Kernel does not build. It treats what it receives as untrusted input:

- it checks every declared hash against the bytes, enforces size and file-count limits and admits an allowlist of content types;
- it refuses a path that escapes the artifact root or collides with what the Kernel serves itself, such as `/_konae/*` and `/runtime.js`;
- it refuses a `routing` other than `"pages"` or `"spa"` with the builders' words;
- it serves the result under the Content Security Policy of ADR-0002.

Owner authentication says who may publish. It never says the bytes are well formed.

### The static publishing rules

`src/lib/static-artifact.js` decides which files of `app/` become the artifact, and `konae build`, the editor's browser build and the app folder's own build script all run it. It is plain JavaScript with no imports for that reason.

- Every `.html` at the root of `app/` and under `pages/` is a page, served without the `pages/` prefix. Files under `public/` are served as they are, without the prefix. Every other file ships only when a page, or a stylesheet a page loads, references it. A reference to a file `app/` does not hold refuses the build with the path.
- A whole-document page keeps its head and loads the SDK from `/runtime.js` through a loader that knows the page's base path. A fragment page runs in the app's document, and its `<title>` and icon links move into that document's head.
- `routing` is `"pages"` by default, where an unknown path answers 404 with `404.html`. `"spa"` answers `index.html` for every unknown path.
- `encodings` names the files that travel as base64, so images and fonts publish.
- esbuild's comments that name a module's path are dropped, so one `app/` gives byte-identical files from any folder and any builder.

### The editor builds in the browser

The editor bundles `app/` with `esbuild-wasm` in its own worker (`src/runtime/browser-build.ts`). Bundling parses and links source without running it, so a dependency cannot act during a build, and the worker is apart from the editor's context, which holds the model provider's keys. A change that does not build cannot be applied or published.

### An app may import npm packages

`app/package.json` lists packages under `dependencies` with exact versions, and that file is the one declaration. The agent and the owner edit it. The editor's build fetches a listed package from esm.sh at that version, with every other listed package pinned through esm.sh's `deps` parameter, and reads only that origin. `konae build` and the folder's build script read the same packages from `node_modules`. The published app loads nothing from esm.sh, and its Content Security Policy does not change.

## Rejected options

- **Build in the Kernel.** Package lifecycle scripts, bundler plugins and module evaluation are code execution inside the process that holds every app's database. Containing them needs a sandbox per build, which is the per-app cost the product exists to avoid.
- **Make a compiler or a project layout the contract.** Either may change without changing what a Deployment needs.
- **Serve unbundled sources.** An app could then use no TypeScript, no imports between files and no packages.
- **Run the builder in the editor's own context.** The editor holds provider keys, and a build is the moment an app's dependencies get near them.
- **A registry of allowed packages with a generated lock.** The build already runs on the owner's machine, so Konae has no reason to choose which libraries an app may use.
- **Carry binary `public/` files through the editor's saved sources.** It would change the saved format and the Kernel's checks on it for one small file. An icon is SVG, as a file or a `data:` URL.

## Consequences

- Build time and memory are the owner's tab, which bounds how large an app built in the editor can be. A larger app builds on its owner's machine and publishes its `dist/`.
- The editor's build and `konae build` use the same package versions but not byte-identical bundles, because esm.sh compiles a package for `es2022` and npm ships what the author published. esm.sh sees the list of packages an app names.
- A page that names a root-relative URL leaves the app's prefix on a path address. The Gate and the release warn about it (ADR-0014).
