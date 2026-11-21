# Functions local demo

This folder is one Konae app. The Kernel serves the pages in `dist/` and runs the Server Actions in `app/actions.json` on the database in `data/`.

## Layout

- `konae.json`: the app's name, visibility (private), module state, contact email and the Kernel version.
- `app/`: the editable sources. It may also hold binary assets. Konae checks the same path, count and decoded-size limits before saving and publishing them.
- `server/`: function bundles and their manifest with SHA-256 hashes. The Kernel never serves these files as static assets. The editable declarations and entries stay in `app/functions.json` and `app/functions/`.
- `dist/`: the built files. `npm run build` writes it from `app/`. Edit `app/`, never `dist/`.
- `data/`: `database.sqlite`, the app accounts in `identity.sqlite`, Usage in `usage.sqlite` and uploads in `files/`. It holds people's accounts, so it never goes into Git.
- `action-contract.md`: every Server Action with its inputs and the columns it returns.
- `actions.d.ts`: the types of `miniapp.actions.call` for each action a page may call. When `app/src/` holds TypeScript, `npm run build` checks it, and what it imports, against them with `tsconfig.json` and stops on an error.

## Rules the Kernel checks

A function exports `handle(request, context)` and returns a `Response`. Use `await context.actions.call(name, input)` for a query or mutation in its declared action allowlist. Functions start off; enable `functions` in `konae.json`. Authenticated functions need `auth`, and a nonempty action allowlist needs `actions`. The editor preview does not execute server functions. Try `konae dev` or a published Deployment with the experimental function runner configured by the operator.

An optional `schema-transforms.json` records ordered data changes between schema fingerprints. The Kernel stores applied history in `data/database.sqlite`; the definition travels with app sources and never ships in `dist/`. The Validation Gate checks its shape, order, bounds and SQL subset. It cannot prove a transition against a real database.

- The page talks to the database only through the named Server Actions in `app/actions.json`. `action-contract.md` lists them.
- A query that reads rows names a LIMIT.
- `schema.sql` is the model. Add tables and columns there; the Kernel applies an added table, column or index on the next publish or start, and a change that drops or retypes one only with the confirmation it names.
- A page loads no script from another origin, uses no inline event handler, and runs no WebAssembly or `eval`. It fetches only from its own origin and the origins `manifest.connects` in `konae.json` names.

## Module settings

| Need | Use | Not |
| --- | --- | --- |
| Turn an app capability on or set its options | `modules_read` and `module_set`, then read it back with `modules_read` | A separate tool or a setting in the Capsule |
| Be listed in search | Turn on `seo` with `module_set`, after `modules_read` | Assume a public app is listed |

## Rules nothing checks

- Text from a row, an upload or a user is written into the page with `textContent` or the template's `escapeHtml`, in attributes too, and is never concatenated into HTML unescaped.

## Run it

1. To write `dist/`, if `package.json` lists an npm package, run `npm install` once, then `npm run build`. The shared local builder builds a template without an install. `dev` writes it without `npm install` when no imported package needs one. If `package.json` has a build script but `node_modules/` is absent, it uses the built-in builder. Once that directory exists, the builder runs `npm run build`. An exported folder already carries its built `dist/` and `server/`. If `app/package.json` declares dependencies, install them with `npm ci --prefix app` when `app/package-lock.json` is present, or `npm install --prefix app` otherwise. Keep the package manifest and lockfile in `app/` so they travel with the sources.
2. Run `konae dev --dir <this folder>`. It serves the app with `data/` and starts again when a file in `app/` changes. From a repository checkout, it uses that checkout's Kernel and builds it with Cargo if absent. The compiled CLI downloads the matching Kernel binary and verifies its checksum unless `KONAE_KERNEL_BIN` names a local one.
3. On a server: copy `.env.example` to `.env`, fill it in, and run `docker compose up -d`. The Kernel prints the app's address, `http://functions-local-demo.localhost:8787` with the example settings.
4. To publish on a Konae installation: `konae login --browser --dir <this folder> --url <installation>`, which signs the owner in through the browser, then `konae publish --dir <this folder>`. `konae push --dir <this folder>` prints what replacing the published app's data with `data/` would change, and with `--yes` it does so after a backup. A pending pull blocks push until you import the pulled data with `dev --reset`.

At startup the Kernel checks the folder as Konae checks a release. It refuses a folder whose `dist/` breaks the publishing rules, whose actions break a rule above, or whose `schema.sql` drops or retypes a column, and says which. To apply a change that loses data, start it once with the `KONAE_CONFIRM_SCHEMA` value it names.

## Commands

Outside Konae, the Kernel's commands manage the app. Each refuses with the same words as the tool in Konae's editor, and prints the answer as JSON. Under Compose, run each as `docker compose exec kernel kernel <command>`. All but `import` work while the Kernel serves.

- `kernel users [--state pending|active|suspended]`: the app accounts, with their IDs.
- `kernel invite <email>...`: email each person an invitation. It needs a mail transport.
- `kernel suspend <user id>`, `kernel resume <user id>`: end an account's sessions and keep it out, or let it back in.
- `kernel signup-policy open|invitation|closed|allowlist [<email or @domain>...]`: who may create an account.
- `kernel api-key create <name> --action <name>... [--files public|private]...`, `kernel api-key list`, `kernel api-key revoke <key id>`: keys for the app's HTTP routes and files. The key appears once.
- `kernel secret set <name> [--use route|call|google]`, with the value on standard input, and `kernel secret list`.
- `kernel sql "<statement>"`: run SQL on `data/database.sqlite`.
- `kernel events --from <YYYY-MM-DD> --to <YYYY-MM-DD>`: notices, calls, scheduled runs, routes and Signals.
- `kernel maintenance on|off`: pause the app, so it takes no writes, signups or sign-ins and holds its email, calls and schedule, or let it run again.
- `kernel enter <email>`: a one-time link, good for 2 minutes, that signs a browser in as that account. Usage and Signals do not count what the session does. With no audit log outside Konae, the Kernel's log records who entered as whom, as `operator:<login name> entered app <id> as <email>.`
- `kernel import <folder> [--confirm-schema <confirmation>]`: replace the app's data with the `data/` of an exported folder, after a backup in `.konae/backups`. Sessions and API keys stay as they are. It needs the Kernel stopped, so under Compose run `docker compose stop kernel`, then `docker compose run --rm kernel kernel import <folder inside the volume>`, then `docker compose start kernel`.

Add `--app app_0832f699-b596-448d-aff7-258b67483ea7` when the Kernel serves more than one app.
