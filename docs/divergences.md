# Named divergences

These are known differences between the places an app runs. They are not bugs to paper over. Say them when they matter.

## Preview and published app

The editor preview applies no Content Security Policy. Inline handlers, other-origin fetches and WebAssembly can still run there. A published app and `konae dev` refuse them.

Attach listeners in JavaScript, and keep scripts and fetches on the app's own origin. A fetch may also reach an origin that `manifest.connects` names.

```html
<!-- Runs in the preview, refused once published -->
<button onclick="save()">Save</button>
```

The policy's `script-src` is `'self'` plus the SHA-256 of each inline script. It names neither `'wasm-unsafe-eval'` nor `'unsafe-eval'`, so a published app and `konae dev` refuse WebAssembly, `eval` and `new Function`. A library that compiles WebAssembly, such as a PDF renderer, works in the preview and then fails in the browser console with `Compiling or instantiating WebAssembly module violates the following Content Security policy directive`. Pick a library that runs without WebAssembly, or do that work outside the app.

## What the preview lacks

The preview has no HTTP routes, schedule timer, notice email, Usage store or call outbox. Try those on `konae dev` or on a published Deployment.

Saved public assets work in the preview, including images in `public/`, linked stylesheets, imported CSS, media conditions and SVG fragments. Programmatic `fetch()` of local saved assets and runtime-created URLs that point to local saved assets need `konae dev` or a Deployment. The preview's missing Content Security Policy still allows fetches to other origins.

## Email on konae dev

`konae dev` sends no email. Account email and notices land in `.konae/kernel/mail-testbox.jsonl` of the folder, and `konae events` lists them. Its test mode captures outbound calls, pauses schedule timers, refuses Google sign-in, and blocks browser connections to other origins. It is not an OS sandbox for npm scripts or other shell commands.

## File size

The editor preview refuses a file over 2 MiB. A Kernel refuses at its own `KONAE_FILE_MAX_BYTES`, 2 MiB unless the operator raised it, and names the number. `konae dev` applies the same 2 MiB per file, and `KONAE_FILE_MAX_BYTES` raises it there. Its quota for all files is 1 TiB unless `KONAE_FILES_QUOTA_BYTES` names another, so `dev --reset` can import the files of a published app.

## Google sign-in

The editor preview has no Google sign-in, and `miniapp.auth.signInWithGoogle()` refuses there. Try it on a published Deployment once its owner has set the app's Google client and turned Google on. [Accounts and access](./accounts-and-access.md#sign-in-with-google) says how.

## npm packages

The editor's build fetches exact npm package versions from esm.sh. `konae build` and an app folder's `npm run build` use the versions installed in `node_modules`. The bundles can differ because esm.sh transforms packages for its CDN.

## Preview database and schema.sql

The editor preview re-runs `schema.sql` on its own test rows and never changes a table it already has. A Kernel derives the change at publish and asks before one that deletes data. When preview rows cannot be checked because the preview schema is behind, the Gate shows `PREVIEW_BEHIND`, names the schema difference and says to try it on `konae dev` or a Deployment. The validation summary says it skipped preview-row checks.

## Gate and publish builders

The Validation Gate checks schema, `actions.json` and `manifest.fileAccess`, and refuses a Capsule whose pages are all whole documents that load no script, as the build does. A page that names a missing file fails at build or release, not at `validate_capsule`. Check every path you write.

## Read next

- [The Gate and the Kernel](./gate-and-kernel.md)
- [The Capsule](./capsule.md)
- [CLI](./cli.md)
