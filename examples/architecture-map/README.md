# The architecture map

A static Konae app that draws the system as a map: the editor, the Validation Gate, the Kernel and what each one holds. It has no Server Actions and no rows.

Publish it from the repository root, with an owner account on the installation:

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/konae-map --url http://localhost:4173
bun run konae -- init --dir /tmp/konae-map --name "Konae architecture" --url http://localhost:4173
cp -R /tmp/konae-map/konae.json /tmp/konae-map/.konae examples/architecture-map/
bun run konae -- publish --dir examples/architecture-map
```

`konae.json` names a project on one Kernel, so git ignores it. To let anyone open the map without an account, set `project.manifest.visibility` to `"public"` and `project.manifest.signupPolicy` to `{ "mode": "closed" }` in it before the first publish.
