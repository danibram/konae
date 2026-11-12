# Política de privacidad

Public Konae app with the privacy policy text. No tables, no Server Actions, no accounts. It answers on the installation at `<domain>/p/politica-de-privacidad/` once its path address holds that name.

## Files

| File | Role |
| --- | --- |
| `index.html` | The page. |
| `styles.css` | The design system in the editor's variant, written by `bun run ui:render`. Do not edit it. |
| `legal.css` | The few rules of this page, on the same tokens. |
| `legal.js` | Points the home and terms links at this installation from `location`. |
| `favicon.svg` | Icon. |
| `schema.sql`, `actions.json` | Empty model and actions. |

## Publish it

Run these from the repository root, with an owner account on the installation.

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/konae-privacy --url https://app.<domain>
bun run konae -- init --dir /tmp/konae-privacy --name "Política de privacidad"
rm -rf /tmp/konae-privacy/app && cp -R examples/politica-de-privacidad /tmp/konae-privacy/app
```

Before the first publish, set `visibility` to `"public"` and `signupPolicy` to `{ "mode": "closed" }` in `/tmp/konae-privacy/konae.json`. Then publish and give it the path name:

```sh
bun run konae -- publish --dir /tmp/konae-privacy
```

```text
path_set  { deploymentId, enabled: true, name: "politica-de-privacidad" }
path_read { deploymentId }
```

The app has no accounts, so `path_set` needs no `acceptSharedOrigin`. Publish [términos y condiciones](../terminos-y-condiciones/README.md) the same way with the name `terminos-y-condiciones`.
