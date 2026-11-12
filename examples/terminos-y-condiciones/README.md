# Términos y condiciones

Public Konae app with the terms text. No tables, no Server Actions, no accounts. It answers on the installation at `<domain>/p/terminos-y-condiciones/` once its path address holds that name.

## Files

| File | Role |
| --- | --- |
| `index.html` | The page. |
| `styles.css` | The design system in the editor's variant, written by `bun run ui:render`. Do not edit it. |
| `legal.css` | The few rules of this page, on the same tokens. |
| `legal.js` | Points the home and privacy links at this installation from `location`. |
| `favicon.svg` | Icon. |
| `schema.sql`, `actions.json` | Empty model and actions. |

## Publish it

Run these from the repository root, with an owner account on the installation.

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/konae-terms --url https://app.<domain>
bun run konae -- init --dir /tmp/konae-terms --name "Términos y condiciones"
rm -rf /tmp/konae-terms/app && cp -R examples/terminos-y-condiciones /tmp/konae-terms/app
```

Before the first publish, set `visibility` to `"public"` and `signupPolicy` to `{ "mode": "closed" }` in `/tmp/konae-terms/konae.json`. Then publish and give it the path name:

```sh
bun run konae -- publish --dir /tmp/konae-terms
```

```text
path_set  { deploymentId, enabled: true, name: "terminos-y-condiciones" }
path_read { deploymentId }
```

The app has no accounts, so `path_set` needs no `acceptSharedOrigin`. Publish [política de privacidad](../politica-de-privacidad/README.md) the same way with the name `politica-de-privacidad`.
