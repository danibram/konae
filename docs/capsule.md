# The Capsule

A Capsule is the editable source of one Project, taken as one unit. The editor, the agent and the CLI all read and write this shape.

## Layout

```text
mini-app/
├── index.html
├── about.html        # every .html at the root is a page
├── 404.html          # answers an unknown path under the default routing
├── pages/            # more pages, published without the prefix
├── public/           # published as they are
├── styles.css
├── app.js            # or src/main.ts
├── src/
│   └── feature.ts
├── package.json      # optional, the npm packages the app imports
├── schema.sql
├── schema.ts         # optional, the Database Model of the editor's Data panel
└── actions.json
```

A folder that the CLI publishes also keeps `konae.json`, which names its project on one Kernel and its manifest: visibility, signup policy and routing.

## What each file does

| File | Role |
| --- | --- |
| `app.js` or `src/main.ts` | The entrypoint. The build bundles its relative imports and the npm packages it imports by name. |
| `index.html`, other `.html` | Pages. Every `.html` at the root and under `pages/` is a page. Pages under `pages/` publish without the prefix. |
| `styles.css` | Styles the pages load. |
| `schema.sql` | The schema the Gate and the Kernel check and run. It may only create tables and indexes, and it must be safe to run twice, because the Kernel runs it again each time it opens the database. |
| `schema.ts` | The Database Model, a Drizzle declaration the editor's Data panel edits. The Kernel derives a reviewed migration from two versions of it with `schema_plan`. An app that never changes a deployed table does not need it. |
| `package.json` | The npm packages the app imports, under `dependencies` with exact versions. A version range is refused. The editor's build reads them from esm.sh, and `konae build` reads them from `node_modules`. The published app holds their code and loads nothing from another origin. |
| `actions.json` | The Server Action declarations. See [Server Actions](./server-actions.md). |
| `404.html` | The page answered for a path the Deployment does not hold, under the default routing mode. |
| `public/` | Files published as they are. |

## From Capsule to Build Artifact

A Deployment serves the Build Artifact built from the Capsule, not the Capsule. Every builder makes it by one rule set.

- Each page gets the SDK injected.
- The files the pages load, and `public/`, are included.
- Binary files travel as base64, named in `encodings`.
- A build that names a file the Capsule does not hold is refused.

The routing mode is `pages` by default. A path the artifact does not hold answers 404 with its `404.html`. In `spa` mode such a path serves `index.html`. The builders and the Kernel refuse any other mode with the same words.

The Gate checks the schema and `actions.json`. A page that names a missing file passes the Gate and fails at build or release. See [Named divergences](./divergences.md).

## What the published page may do

A published app runs under the Kernel's Content Security Policy. Its own inline scripts and same-origin files load. Inline event handler attributes such as `onclick="..."` and other origins do not. Attach listeners in JavaScript and keep scripts and fetches on the app's own origin. The editor preview applies no such policy, so code that works there can still fail once published.

## Where the sources live

Every project is one kind. The editor saves its sources in Konae, and a change there passes the Validation Gate before it is saved. `konae init` writes the same files into a folder, and `konae publish` sends them to the same project. A publish from a folder does not change the sources the editor opens.

## Read next

- [Server Actions](./server-actions.md)
- [The Gate and the Kernel](./gate-and-kernel.md)
- [Edit files with the CLI](./guides/edit-files-with-the-cli.md)
- [Named divergences](./divergences.md)
