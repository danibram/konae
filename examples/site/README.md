# The Konae site

The home, the docs and `llms.txt` of Konae, published as one Konae app. On konae.es it answers as the app's custom domain: the home at `/`, the Spanish home at `/es/`, the docs at `/docs/` and the index for agents at `/llms.txt`. The markdown under `docs/` is the only source of the docs.

The home's hero is a drawing of the editor that builds an example app when it scrolls into view, with a working list the visitor can add a name to. Below it come how an app is made, what every app has, the move of an app to another server, the two ways of working on one app, the example apps, a developer section, the questions people ask before requesting an invitation, and the steps to start. The editor in the hero is a recreation in the editor's own words, not a capture, so it needs updating when the editor's labels change.

## Addresses

| Link | Where it points |
| --- | --- |
| Editor and owner MCP | `app.<host>`, where `<host>` is the address the page answers on |
| Docs and `llms.txt` | `/docs/` and `/llms.txt` on the same host |
| Invitation, privacy policy, terms | `<apps>/p/invitation/`, `<apps>/p/politica-de-privacidad/`, `<apps>/p/terminos-y-condiciones/` |

`<apps>` is the `data-apps` attribute of `<html>` in `index.html`, the domain where this installation's path addresses answer (`KONAE_DOMAIN`). Without it the page uses its own host, which fits an installation that serves this site through the path name `konae-home` instead of a custom domain. [Invitation](../invitation/README.md) and the two legal pages are separate apps.

## Files

| File | Role |
| --- | --- |
| `index.html` | The home. |
| `pages/es/index.html` | The Spanish home at `/es/`. `bun run home:site` writes it from `index.html` and `i18n.js`. |
| `home.js`, `home.css`, `i18n.js` | The home's script, rules and Spanish text. Raise `?v=` in `index.html` when `home.css` or `home.js` changes, because a proxy may let browsers keep CSS and JavaScript for hours. |
| `query-bound.ts`, `sql-tokens.ts` | Copies of `src/lib/query-bound.ts` and `src/lib/sql-tokens.ts`, so the row-limit box runs the editor's own check. `bun run ui:render` writes them. |
| `styles.css` | The design system, written by `bun run ui:render`. The home and the docs share it. |
| `docs.template` | The shell of every docs page: header, navigation, search box and rendered article. Its `?v=` is a hash `bun run docs:site` derives from the shared files. |
| `render.js`, `slug.js` | Render a page body with the pinned `marked`, and give headings the ids GitHub gives them. The generator and the browser share them. |
| `shell.js`, `site.css` | The docs pages' script and styles: links between pages, diagrams, table of contents and search. |
| `schema.sql`, `actions.json` | The `sections` table and `docs.search`, an anonymous query with a `LIMIT`. |
| `public/card.png`, `public/docs/card.png` | Card images, editable as the `.svg` beside each. `bash scripts/render-card-images.sh` rebuilds them. |

`bun run docs:site` writes `pages/docs/`, `public/docs/content/`, `public/docs/vendor/`, `public/llms.txt` and `seed.sql` from `docs/` and `docs/site.json`. Git ignores all of them.

## Add or change a docs page

Edit the markdown under `docs/`. A new page also needs an entry in `docs/site.json`, which holds the navigation. Write links to other pages as relative `.md` paths, such as `./capsule.md#layout`, so they work in the repository and on the site. Write a diagram as a `mermaid` code block and say the same in the paragraph under it, because an agent reads the markdown and search skips the block. `bun run docs:site` copies `@mermaid-js/tiny` into `public/docs/vendor/`, about 2.9 MB that only a page with a diagram loads. A page that only keeps an old address alive goes under `unlisted` in `docs/site.json`.

`scripts/docs-site.test.ts` fails when a page in the navigation is missing, when a relative link names a file or a heading that does not exist, or when the MCP reference misses a tool the Kernel registers.

## Publish it

Run these from the repository root, with an owner account on the installation.

```sh
(cd examples/site && bun install)
bun run home:site
bun run docs:site
bun run konae -- init --dir /tmp/konae-site --name "Konae site"
rsync -a --delete --exclude node_modules --exclude .konae --exclude konae.json examples/site/ /tmp/konae-site/app/
(cd /tmp/konae-site/app && bun install --frozen-lockfile)
bun run konae -- login --browser --dir /tmp/konae-site --url https://app.<domain>
```

Before the first publish, set `visibility` to `"public"` and `signupPolicy` to `{ "mode": "closed" }` in `/tmp/konae-site/konae.json`. Then publish and load the search rows:

```sh
bun run konae -- publish --dir /tmp/konae-site
bun run docs:site --seed /tmp/konae-site
```

To answer on its own domain, run `bun run konae -- domain --set konae.es --dir /tmp/konae-site`, add the TXT record it names, and run it again. An installation without a domain for the site gives it the path name `konae-home` instead, so it answers `<domain>/`. Turn on `seo` with `module_set` and set its image to `card.png`.

## Limits

A page cannot live at `/mcp`, because every Deployment answers that path with its runtime endpoint. The reference is at `/docs/reference`.
