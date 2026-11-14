# The docs site

The Konae docs, published as a Konae app. The markdown under `docs/` is the only source. This app publishes those files as they are and renders them in the browser, so the page a person reads and the `.md` an agent fetches are the same text.

## What it shows

| Piece | Where |
| --- | --- |
| Static files published as they are | `public/content/*.md`, `public/llms.txt` |
| One page per address under `pages/` | Each page is the same shell with a different `data-route` |
| An npm package in the bundle | `shell.js` imports `marked`, pinned in `package.json` |
| A large script a page loads only when it needs it | `public/vendor/mermaid.js`, which draws the diagrams, loaded by the pages that hold one |
| An anonymous Server Action | `docs.search` in `actions.json`, a query with a `LIMIT` over the `sections` table |
| A public app with no accounts | `visibility: "public"` and signup `closed` in `konae.json` |
| Relative links only | The app works on its hostname and on a path address |

## Files

| File | Role |
| --- | --- |
| `page.template` | The shell of every page: header, navigation, search box and an empty article. It asks for the styles and `shell.js` with `?v=`, a name `bun run docs:site` derives from their bytes, so a proxy that lets browsers keep CSS and JavaScript for hours serves a change at once. |
| `shell.js` | Fetches `content/<route>.md`, renders it with `marked`, gives headings the ids GitHub gives them, points `.md` links at pages, draws each `mermaid` block as a diagram, builds the table of contents and runs the search. |
| `slug.js` | The heading id rule, shared with the generator. |
| `site.css` | Styles. |
| `schema.sql`, `actions.json` | The `sections` table and the search action. |

`bun run docs:site` writes the rest from `docs/` and `docs/site.json`, and git ignores all of it: `index.html`, `pages/`, `public/content/`, `public/vendor/`, `public/llms.txt` and `seed.sql`.

## Publish it

Run these from the repository root, with an owner account on the installation.

```sh
(cd examples/docs-site && bun install)
bun run docs:site
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/konae-docs --url http://localhost:4173
bun run konae -- init --dir /tmp/konae-docs --name "Konae docs"
rm -rf /tmp/konae-docs/app && cp -R examples/docs-site /tmp/konae-docs/app
```

`init` writes an app folder in `/tmp/konae-docs`, and the last line puts this app's sources in its `app/`. The first publish registers the project on the installation. Before the first publish, set `visibility` to `"public"` and `signupPolicy` to `{ "mode": "closed" }` in `/tmp/konae-docs/konae.json`, so anyone can read the docs and nobody can create an account.

```sh
bun run konae -- publish --dir /tmp/konae-docs
bun run docs:site --seed /tmp/konae-docs
```

`publish` sends the pages and the markdown. `--seed` loads the search rows, one per `## ` section, with `database_execute`, and reads the count back. When a page changes, run `bun run docs:site`, copy `examples/docs-site` into `/tmp/konae-docs/app` again, and run both again.

## Add or change a page

Edit the markdown under `docs/`. A new page also needs an entry in `docs/site.json`, which holds the navigation. Write links to other pages as relative `.md` paths, such as `./capsule.md#layout`, so they work in the repository and on the site. Write a diagram as a `mermaid` code block and say the same in the paragraph under it, because an agent reads the markdown and search skips the block. Keep a diagram to a few nodes a row, top to bottom, so it stays readable in the page's column. `bun run docs:site` copies `@mermaid-js/tiny` from `node_modules` into `public/vendor/`, about 2.9 MB that only a page with a diagram loads. A link to a file outside the site opens the repository when `repository` in `docs/site.json` holds its URL, and shows as plain text when it is empty. A page that only keeps an old address alive goes under `unlisted` in `docs/site.json`, so it answers at its route and appears in no menu.

`scripts/docs-site.test.ts` fails when a page in the navigation is missing, when a relative link names a file or a heading that does not exist, or when the MCP reference misses a tool the Kernel registers.

## Limits of this example

A page cannot live at `/mcp`, because every Deployment answers that path with its runtime endpoint. The reference is at `/reference`, and `/mcp-guide`, where the old MCP guide was, is a short page that lists the reference pages.
