# The architecture map

A static Konae app that draws the system as a map: the editor, the Validation Gate, the Kernel and what each one holds. It has no Server Actions and no rows.

Publish it from the repository root, with an owner account on the installation:

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/konae-map --url http://localhost:4173
bun run konae -- init --dir /tmp/konae-map --name "Konae architecture"
rm -rf /tmp/konae-map/app && cp -R examples/architecture-map /tmp/konae-map/app
bun run konae -- publish --dir /tmp/konae-map
```

`init` writes an app folder, and the second line puts this app's sources in its `app/`. To let anyone open the map without an account, set `visibility` to `"public"` and `signupPolicy` to `{ "mode": "closed" }` in `/tmp/konae-map/konae.json` before the first publish.
