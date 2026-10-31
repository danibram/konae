# The docs site

The Konae docs, published as a Konae app. The markdown under `docs/` is the only source. This app publishes those files as they are and renders them in the browser, so the page a person reads and the `.md` an agent fetches are the same text.

## What it shows

| Piece | Where |
| --- | --- |
| Static files published as they are | `public/content/*.md`, `public/llms.txt` |
| One page per address under `pages/` | Each page is the same shell with a different `data-route` |
| An npm package in the bundle | `shell.js` imports `marked`, pinned in `package.json` |
| An anonymous Server Action | `docs.search` in `actions.json`, a query with a `LIMIT` over the `sections` table |
| A public app with no accounts | `visibility: "public"` and signup `closed` in `konae.json` |
| Relative links only | The app works on its hostname and on a path address |

## Files

| File | Role |
| --- | --- |
| `page.template` | The shell of every page: header, navigation, search box and an empty article. |
| `shell.js` | Fetches `content/<route>.md`, renders it with `marked`, gives headings the ids GitHub gives them, points `.md` links at pages, builds the table of contents and runs the search. |
| `slug.js` | The heading id rule, shared with the generator. |
| `site.css` | Styles. |
| `schema.sql`, `actions.json` | The `sections` table and the search action. |

`bun run docs:site` writes the rest from `docs/` and `docs/site.json`, and git ignores all of it: `index.html`, `pages/`, `public/content/`, `public/llms.txt` and `seed.sql`.

## Publish it

Run these from the repository root, with an owner account on the installation.

```sh
(cd examples/docs-site && bun install)
bun run docs:site
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/konae-docs --url http://localhost:4173
bun run konae -- init --dir /tmp/konae-docs --name "Konae docs" --url http://localhost:4173
cp -R /tmp/konae-docs/konae.json /tmp/konae-docs/.konae examples/docs-site/
```

`init` needs an empty folder, so it writes the project in `/tmp/konae-docs` and the last line brings its `konae.json` and its credential here. The first publish registers the project on the installation. `konae.json` names a project on one Kernel, so git ignores it. Before the first publish, set `project.manifest.visibility` to `"public"` and `project.manifest.signupPolicy` to `{ "mode": "closed" }` in it, so anyone can read the docs and nobody can create an account.

```sh
bun run konae -- publish --dir examples/docs-site
bun run docs:site --seed
```

`publish` sends the pages and the markdown. `--seed` loads the search rows, one per `## ` section, with `database_execute`, and reads the count back. Run both again when a page changes.

## Add or change a page

Edit the markdown under `docs/`. A new page also needs an entry in `docs/site.json`, which holds the navigation. Write links to other pages as relative `.md` paths, such as `./capsule.md#layout`, so they work in the repository and on the site. A link to a file outside the site opens the repository when `repository` in `docs/site.json` holds its URL, and shows as plain text when it is empty.

`scripts/docs-site.test.ts` fails when a page in the navigation is missing, when a relative link names a file or a heading that does not exist, or when the MCP reference misses a tool the Kernel registers.

## Limits of this example

A page cannot live at `/mcp`, because every Deployment answers that path with its runtime endpoint. The MCP guide is therefore at `/mcp-guide`.

Mermaid diagrams in the markdown show as code.
