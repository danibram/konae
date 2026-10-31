# The home

The page an installation answers on its bare domain. It says what Konae does, shows an example app moving between two servers, runs the editor's row-limit check on a statement the visitor can edit, and links to the editor on `app.<domain>` and to the docs on `<domain>/p/docs/`. It is a public Konae app with no tables, no Server Actions and no accounts. Everything it shows runs in the browser.

## How the Kernel finds it

The Deployment whose path address holds the name `konae-home` answers `<domain>/` and every other path on `<domain>` outside `/p/` ([ADR-0024](../../docs/adr/0024-an-installation-names-its-hosts-from-one-domain-and-an-app-may-answer-on-a-path.md)). With no app on that name, `<domain>/` answers not found. There is no variable to set and nothing to restart.

## Files

| File | Role |
| --- | --- |
| `index.html` | The page. |
| `styles.css` | The design system in the editor's variant, written by `bun run ui:render`. Do not edit it. |
| `home.css` | The few rules of this page, on the same tokens. |
| `home.js` | Points the editor, MCP and docs links at this installation from `location`, because the page answers on `<domain>/`, `<domain>/p/konae-home/` and its own hostname. On a custom domain it cannot tell the installation's domain and treats the custom domain as one. |
| `query-bound.ts`, `sql-tokens.ts` | Copies of `src/lib/query-bound.ts` and `src/lib/sql-tokens.ts`, so the row-limit box runs the editor's own check. `bun run ui:render` writes them, and `src/lib/templates/app-ui.test.ts` fails when they drift. |

## Publish it

Run these from the repository root, with an owner account on the installation.

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/konae-home --url https://app.<domain>
bun run konae -- init --dir /tmp/konae-home --name "Konae home" --url https://app.<domain>
cp -R /tmp/konae-home/konae.json /tmp/konae-home/.konae examples/home/
```

Before the first publish, set `project.manifest.visibility` to `"public"` and `project.manifest.signupPolicy` to `{ "mode": "closed" }` in `konae.json`. Then publish, give it the name and read it back over MCP:

```sh
bun run konae -- publish --dir examples/home
```

```text
path_set  { deploymentId, enabled: true, name: "konae-home" }
path_read { deploymentId }
```

The app has no accounts, so `path_set` needs no `acceptSharedOrigin`. Publish [the docs](../docs-site/README.md) the same way and give them the name `docs`, which the links on this page expect.
