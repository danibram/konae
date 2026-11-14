# Named divergences

These are known differences between the places an app runs. They are not bugs to paper over. Say them when they matter.

## Preview and published app

The editor preview applies no Content Security Policy. Inline handlers and other-origin fetches can still run there. A published app refuses them.

Attach listeners in JavaScript, and keep scripts and fetches on the app's own origin.

```html
<!-- Runs in the preview, refused once published -->
<button onclick="save()">Save</button>
```

## What the preview lacks

The preview has no HTTP routes, schedule timer, notice email, Usage store or call outbox. Try those on `konae dev` or on a published Deployment.

## Email on konae dev

`konae dev` sends no email. Account email and notices land in `.konae/kernel/mailbox.jsonl` of the folder, and `konae events` lists them.

## File size

The editor preview refuses a file over 2 MiB. A Kernel refuses at its own `KONAE_FILE_MAX_BYTES`, 2 MiB unless the operator raised it, and names the number. `konae dev` applies the same 2 MiB per file, and `KONAE_FILE_MAX_BYTES` raises it there. Its quota for all files is 1 TiB unless `KONAE_FILES_QUOTA_BYTES` names another, so `dev --reset` can import the files of a published app.

## Google sign-in

The editor preview has no Google sign-in, and `miniapp.auth.signInWithGoogle()` refuses there. Try it on a published Deployment once its owner has set the app's Google client and turned Google on. [Accounts and access](./accounts-and-access.md#sign-in-with-google) says how.

## npm packages

The editor's build reads npm packages from esm.sh. `konae build` and an app folder's `npm run build` read them from `node_modules`. Both use the exact versions in `package.json`, but the two bundles are not byte-identical.

## Preview database and schema.sql

The editor preview re-runs `schema.sql` on its own test rows and never changes a table it already has. A Kernel derives the change at publish and asks before one that deletes data. When the preview lacks a table or column `schema.sql` declares, the Gate warns and passes. Try such a change on `konae dev` or a published Deployment.

## Gate and publish builders

The Validation Gate checks schema and `actions.json`. A page that names a missing file fails at build or release, not at `validate_capsule`. Check every path you write.

## Read next

- [The Gate and the Kernel](./gate-and-kernel.md)
- [The Capsule](./capsule.md)
- [CLI](./cli.md)
