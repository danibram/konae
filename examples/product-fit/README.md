# Product fit examples

These eight examples exercise Konae's current contracts through an isolated Kernel and real MCP requests. The sources came from the interrupted Fable product trial. The replay creates new projects, accounts and files each time. It copies the sources into a temporary directory and removes that directory and its Kernel when it exits.

Run from the repository root with Bun, Cargo and Google Chrome installed and the repository dependencies available.

```sh
bun run examples/product-fit/smoke.ts
```

The default port is 19410. Set `KONAE_PRODUCT_FIT_PORT` to another unused local port when needed. The replay builds the Kernel with Cargo's offline mode. It does not connect to an existing installation, inherit Konae credentials or use production data. Passwords and the setup key are random and exist only for that run.

The runner sets the feed's signing secret, publishes all eight examples, then checks ten groups of behavior. It prints each result and a JSON report. It checks HTTP and MCP behavior, then opens every example in Google Chrome through Playwright and fails on any Content Security Policy violation or page error. Chrome must be installed; set `KONAE_PRODUCT_FIT_BROWSER=0` to skip the browser pass. It does not assert browser layout or screen reader behavior.

| Example | Behavior checked | Current cost or limit |
| --- | --- | --- |
| `launch` | Anonymous waitlist signup, count, and a repeated address refused with the guard `That address is already on the list.` in any letter case | Browser forms must map their fields to declared action inputs. The statement lowercases the address itself, since the page's lowercasing is only a courtesy |
| `journal` | Two accounts, private rows, a cross-account delete refused with its guard | Each action must include its ownership predicate |
| `board` | Private shell, allowlist, four users, member/admin permissions, each refusal with its guard's sentence | The owner seeds the first admin with SQL. Promotion names a member's row, because a member picks their own display name and two may pick the same one. The row ID is SQLite's implicit `rowid`, since `members` keys on the user ID; a copy that deletes members should declare `id INTEGER PRIMARY KEY`, which a `VACUUM` never renumbers. `conform` cannot reach the guard of `cards.create` with a member's session; the replay reaches it with an account that has not joined |
| `portfolio` | Authenticated upload and checksum-verified download, public gallery | The photo upload stays private to the potter, and a visitor's piece list carries no file ID; the potter reads hers with `pieces.files`. Visitors read the inline copy. A file uploaded as public can be read without that copy |
| `poll` | Anonymous votes, a choice outside the four days refused with its guard, totals and hourly aggregation | Browser local storage discourages repeat votes but cannot prevent them. The hours are grouped in UTC and labelled in the reader's time |
| `feed` | Anonymous JSON-RPC reads and writes, rate limiting, a signed ping sent with `curl` to `POST /hooks/pings`, an unsigned and a replayed ping refused, `GET /api/incidents` as JSON, a status badge embedded from `/_konae/files/<id>`, a private badge read by an API key and refused to a visitor | A sender must compute Konae's signature (`t=<seconds>,n=<nonce>,v1=<HMAC-SHA256 over method, path, query and body>`); a service with its own fixed scheme needs an adapter. Unknown POST paths such as `/hook` return 405 |
| `docs` | Multi-page artifact from `konae build` (`pages/guide/` and `public/`), a guide page, SQL-backed search | Missing paths answer 404, `/guide/install` serves the guide and `sitemap.xml` is `application/xml` since 2026-11-06 |
| `probe` | Nullable search input, omitted/null/filtered values and action declarations | This is a contract probe rather than an end-user app |

The runner also fails nine release plans through the CLI, repairs the schema, and publishes with the same credential. That checks upload cleanup against the Kernel's real transfer quota. It separately verifies that a mutation with `RETURNING` fails release planning with a useful error. Last, it writes each example's Action Contract and runs the conformance checks of `bun run konae -- conform` against the Kernel serving it, then runs the CLI once from a contract file on disk; every check must pass, and the only skip allowed is the board's `cards.create` guard. The run on 2026-11-07 passed 174 checks.

Since 2026-11-07 every mutation a person can be refused by declares a guard, so the page shows the app's sentence instead of judging `{ changes: 0 }` itself, and each page reads a failure through one `reason(error)` helper: a refusal shows its message, `rate-limited` and an unreachable Kernel show a sentence. The pages keep the plain styles they were built with; the [showcase apps](../showcase/README.md) use the design system's tokens and follow the system theme.

## Use one example in another project

The fixture folders contain source files only. They carry no existing project ID or server URL. Create a new project with the CLI, then copy an example's sources over its template while retaining the generated `konae.json`.

```sh
bun run konae -- init --dir /tmp/my-journal --name "My journal" --url http://localhost:4173
cp -R examples/product-fit/journal/. /tmp/my-journal/
bun run konae -- dev --dir /tmp/my-journal --port 8796
```

`init` needs an owner credential, as described in [the MCP guide](../../docs/mcp.md). The local development command uses its own accounts and data. The docs fixture publishes with the normal CLI builder: its guide pages sit under `pages/` and its `sitemap.xml` and `robots.txt` under `public/`.

The gallery's inline image is the copy a visitor sees when the photo stayed private to the uploader. A file uploaded as public can be read without that copy. The team board likewise demonstrates authorization with SQL predicates; display names are not verified identity.
