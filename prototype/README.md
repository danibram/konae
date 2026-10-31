# Prototypes

Two Littlebox apps, published on a Kernel through MCP the way any owner's agent would publish them. None of them is part of the editor bundle. The component gallery stays in the editor at `/design-system`, built from the editor's own components.

| Folder | App |
| --- | --- |
| `architecture/` | The system map. Static, no actions. |
| `docs/` | The docs site: 24 pages under `pages/`, `public/llms.txt`, and search through the anonymous `pages.search` query. |

## Publish one

`init` needs no credential, but `publish` does, so run `bun run littlebox -- login` first (or set `LITTLEBOX_MCP_TOKEN` to an owner token):

```sh
bun run littlebox -- init --dir /tmp/lb-docs --name "Littlebox docs" --url http://localhost:4173
cp /tmp/lb-docs/littlebox.json prototype/docs/
bun run littlebox -- publish --dir prototype/docs
```

`littlebox.json` names a project on one Kernel, so git ignores it. To let anyone open the apps without an account, set `project.manifest.visibility` to `public` and `signupPolicy.mode` to `closed` in it before the first publish.

The docs search reads the `pages` table. After the first publish, load `docs/seed.sql` with MCP `database_execute` (`operation: "sql"`, `input.sql` = the file) and read it back with `SELECT count(*) FROM pages LIMIT 1`. Regenerate `seed.sql` when a page's text changes.

A page cannot live at `/mcp`: every Deployment answers that path with its runtime MCP endpoint, so the MCP reference page is `/mcp-tools`.
