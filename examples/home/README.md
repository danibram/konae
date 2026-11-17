# The home

The page an installation answers on its bare domain. Its hero is a drawing of the editor that builds an example app when it scrolls into view, with a working list the visitor can add a name to. Below it come how an app is made in three steps, what every app has, the move of an app to another server with the files an export holds, the two ways of working on one app, the example apps, a developer section (the CLI, the MCP endpoint and running an installation behind tabs, then the editor's row-limit check on a statement the visitor can edit), the questions people ask before requesting an invitation, and the steps to start. It links to the editor on `app.<domain>`, to the docs on `<domain>/p/docs/` and to [the invitation requests](../invitation/README.md) on `<domain>/p/invitation/`. Its footer links the privacy policy and the terms when `<domain>/p/politica-de-privacidad/` and `<domain>/p/terminos-y-condiciones/` answer. The animation names the installation's own domain as the first server. It is a public Konae app with no tables, no Server Actions and no accounts. Everything it shows runs in the browser. The editor in the hero is a recreation in the editor's own words, not a capture, so it needs updating when the editor's labels change.

## How the Kernel finds it

The Deployment whose path address holds the name `konae-home` answers `<domain>/` and every other path on `<domain>` outside `/p/` ([ADR-0014](../../docs/adr/0014-an-app-answers-on-its-hostname-a-custom-domain-or-a-path.md)). With no app on that name, `<domain>/` answers not found. There is no variable to set and nothing to restart.

## Files

| File | Role |
| --- | --- |
| `index.html` | The page. |
| `styles.css` | The design system in the editor's variant, written by `bun run ui:render`. Do not edit it. |
| `home.css` | The few rules of this page, on the same tokens. |
| `home.js` | Points the editor, MCP and docs links at this installation from `location`, because the page answers on `<domain>/`, `<domain>/p/konae-home/` and its own hostname. On a custom domain it cannot tell the installation's domain and treats the custom domain as one. |
| `pages/es/index.html` | Generated Spanish page at `/es/`. `bun run home:site` reads the English page and the translations in `i18n.js`, so each sentence has one source. |
| `public/card.png` | The home card image. `public/card.svg` is its editable source. Run `bash scripts/render-card-images.sh` to rebuild both app cards. Set the `seo` module's `image` option to `card.png`. |
| `?v=` on `home.css` and `home.js` | A proxy in front of the installation may let browsers keep CSS and JavaScript for hours, while the Kernel serves the page itself with `no-cache`. Raise the number in `index.html` when either file changes, so a returning visitor does not get the new page with the old files. |
| `i18n.js` | The page's Spanish text, keyed by each element's `data-t`, and the text the script writes in both languages. The English text is the markup. |
| `query-bound.ts`, `sql-tokens.ts` | Copies of `src/lib/query-bound.ts` and `src/lib/sql-tokens.ts`, so the row-limit box runs the editor's own check. `bun run ui:render` writes them, and `src/lib/templates/app-ui.test.ts` fails when they drift. |

## Publish it

Run these from the repository root, with an owner account on the installation. Generate the Spanish page before copying the app folder.

```sh
bun run home:site
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/konae-home --url https://app.<domain>
bun run konae -- init --dir /tmp/konae-home --name "Konae home"
rm -rf /tmp/konae-home/app && cp -R examples/home /tmp/konae-home/app
```

`init` writes an app folder, and the second line puts this app's sources in its `app/`. Before the first publish, set `visibility` to `"public"` and `signupPolicy` to `{ "mode": "closed" }` in `/tmp/konae-home/konae.json`. Then publish, give it the name and read it back over MCP:

```sh
bun run konae -- publish --dir /tmp/konae-home
```

```text
bun run konae -- tool path_set '{"projectId":"<home id>","enabled":true,"name":"konae-home"}' --dir /tmp/konae-home
bun run konae -- tool path_read '{"projectId":"<home id>"}' --dir /tmp/konae-home
```

The app has no database, accounts or file store. Turn on `seo` with `module_set` and set its image to `card.png`. Publish [the docs](../docs-site/README.md) the same way and give them the name `docs`, which the links on this page expect, and [the invitation requests](../invitation/README.md) with the name `invitation`.
