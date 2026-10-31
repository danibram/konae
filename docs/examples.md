# Example apps

The repository holds three sets of example apps, the docs site, the installation home and an architecture map. They are the sources the Kernel is tested against, so each one also shows a rule the Kernel enforces.

## Showcase

Six apps in `examples/showcase/`. Each has a `replay.ts` that starts its own Kernel, publishes the app and runs every flow and refusal.

- `examples/showcase/starter`, the typical app in one, and the one to copy first. It has a public landing and API, verified open signup, and member, team and admin areas.
- `examples/showcase/inbox`, a private operations inbox fed by signed webhooks.
- `examples/showcase/club`, a members-only book club with private and shared files.
- `examples/showcase/gallery`, a public portfolio with a contact form and a JSON feed.
- `examples/showcase/habits`, a personal tracker used from two devices.
- `examples/showcase/booking`, a room board with a public view and staff sign-in.

```sh
bun run examples/showcase/replay-all.ts
```

## Product fit

Eight apps in `examples/product-fit/`. A smoke run publishes all eight on an isolated Kernel and checks them over HTTP and MCP, then opens each one in Chrome.

- `examples/product-fit/launch`, an anonymous waitlist with a guard on repeat addresses.
- `examples/product-fit/journal`, two accounts with private rows.
- `examples/product-fit/board`, a private team board with an allowlist and member and admin permissions.
- `examples/product-fit/portfolio`, an authenticated upload and a public gallery.
- `examples/product-fit/poll`, anonymous votes with totals.
- `examples/product-fit/feed`, anonymous JSON-RPC reads and writes, and a signed ping.
- `examples/product-fit/docs`, a multi-page artifact built from `pages/` and `public/`.
- `examples/product-fit/probe`, a contract probe rather than an end-user app.

```sh
bun run examples/product-fit/smoke.ts
```

To start from one of them, create a new project with `konae init`, then copy the example's sources over the template and keep the generated `konae.json`.

```sh
bun run konae -- init --dir /tmp/my-journal --name "My journal" --url http://localhost:4173
cp -R examples/product-fit/journal/. /tmp/my-journal/
bun run konae -- dev --dir /tmp/my-journal --port 8796
```

## Probes

Small Capsules in `examples/probes/` that decide the cells of the capability matrix no showcase replay proves. Examples are `notify` (email on an event), `schedule` (work run by the timer) and `calls` (a tasks app that escalates to a desk app). `examples/probes/` holds eleven.

```sh
bun run examples/probes/replay-all.ts
```

## The docs site

`examples/docs-site/` is the app that serves these docs. It publishes the markdown under `docs/` as static files, renders it in the browser with an npm package, and searches it with one anonymous Server Action. Its `README.md` has the commands to publish it.

## The home and the architecture map

`examples/home/` is the page an installation answers on its bare domain once an app's path address holds the name `konae-home`. `examples/architecture-map/` is a static map of the system. Each has a `README.md` with the commands to publish it.

## Read next

- [Server Actions](./server-actions.md)
- [First app from the editor](./guides/first-app-from-the-editor.md)
- [Pick the smallest piece](./pieces.md)
