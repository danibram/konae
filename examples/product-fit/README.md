# Product fit examples

These eight examples exercise Littlebox's current contracts through an isolated Kernel and real MCP requests. The sources came from the interrupted Fable product trial. The replay creates new projects, accounts and files each time. It copies the sources into a temporary directory and removes that directory and its Kernel when it exits.

Run from the repository root with Bun, Cargo and Google Chrome installed and the repository dependencies available.

```sh
bun run examples/product-fit/smoke.ts
```

The default port is 19410. Set `LITTLEBOX_PRODUCT_FIT_PORT` to another unused local port when needed. The replay builds the Kernel with Cargo's offline mode. It does not connect to an existing installation, inherit Littlebox credentials or use production data. Passwords and the setup key are random and exist only for that run.

The runner sets the feed's signing secret, publishes all eight examples, then checks ten groups of behavior. It prints each result and a JSON report. It checks HTTP and MCP behavior, then opens every example in Google Chrome through Playwright and fails on any Content Security Policy violation or page error. Chrome must be installed; set `LITTLEBOX_PRODUCT_FIT_BROWSER=0` to skip the browser pass. It does not assert browser layout or screen reader behavior.

| Example | Behavior checked | Current cost or limit |
| --- | --- | --- |
| `launch` | Anonymous waitlist signup, count and duplicate rejection | Browser forms must map their fields to declared action inputs |
| `journal` | Two accounts, private rows, blocked cross-account delete | Each action must include its ownership predicate |
| `board` | Private shell, allowlist, three users, member/admin permissions | The owner seeds the first admin with SQL; unauthorized mutations return zero changes |
| `portfolio` | Authenticated upload and checksum-verified download, public gallery | The photo upload stays private to the potter. Visitors read the inline copy. A file uploaded as public can be read without that copy |
| `poll` | Anonymous votes, allowed choices, totals and hourly aggregation | Browser local storage discourages repeat votes but cannot prevent them |
| `feed` | Anonymous JSON-RPC reads and writes, rate limiting, a signed ping sent with `curl` to `POST /hooks/pings`, an unsigned and a replayed ping refused, `GET /api/incidents` as JSON, a status badge embedded from `/_littlebox/files/<id>`, a private badge read by an API key and refused to a visitor | A sender must compute Littlebox's signature (`t=<seconds>,n=<nonce>,v1=<HMAC-SHA256 over method, path, query and body>`); a service with its own fixed scheme needs an adapter. Unknown POST paths such as `/hook` return 405 |
| `docs` | Multi-page artifact from `littlebox build` (`pages/guide/` and `public/`), a guide page, SQL-backed search | Missing paths answer 404, `/guide/install` serves the guide and `sitemap.xml` is `application/xml` since 2026-11-06 |
| `probe` | Nullable search input, omitted/null/filtered values and action declarations | This is a contract probe rather than an end-user app |

The runner also fails nine release plans through the CLI, repairs the schema, and publishes with the same credential. That checks upload cleanup against the Kernel's real transfer quota. It separately verifies that a mutation with `RETURNING` fails release planning with a useful error. Last, it writes each example's Action Contract and runs the conformance checks of `bun run littlebox -- conform` against the Kernel serving it, then runs the CLI once from a contract file on disk; every check must pass and none may be skipped.

## Use one example in another project

The fixture folders contain source files only. They carry no existing project ID or server URL. Create a new External Project with the CLI, then copy an example's sources over its template while retaining the generated `littlebox.json`.

```sh
bun run littlebox -- init --dir /tmp/my-journal --name "My journal" --url http://localhost:4173
cp -R examples/product-fit/journal/. /tmp/my-journal/
bun run littlebox -- dev --dir /tmp/my-journal --port 8796
```

`init` needs an owner credential, as described in [the MCP guide](../../docs/mcp.md). The local development command uses its own accounts and data. The docs fixture publishes with the normal CLI builder: its guide pages sit under `pages/` and its `sitemap.xml` and `robots.txt` under `public/`.

The gallery's inline image is the copy a visitor sees when the photo stayed private to the uploader. A file uploaded as public can be read without that copy. The team board likewise demonstrates authorization with SQL predicates; display names are not verified identity.
