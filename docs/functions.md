# Server functions

Server functions let an app handle HTTP requests and cron jobs with bundled JavaScript. The Kernel owns authentication, app data, and the declared Server Actions a handler may call. Functions are experimental. The [operator guide](./operate.md#typescript-function-workers) describes runner setup and Linux requirements.

## Add a function

For HTTP Functions in an app without cron jobs, turn on `functions` in the app's module settings. A function with `access: "authenticated"` also requires `auth`. A function that names Server Actions requires `actions`. For a local run, set these modules in `konae.json` under `modules`.

An anonymous function also requires a public Deployment. A private Deployment requires an app user session, even for a function whose `access` is `anonymous`.

For example, an anonymous function with no action calls needs only this local module setting:

```json
{
  "modules": {
    "functions": { "on": true }
  }
}
```

Add `functions.json` to `app/`. This declaration exposes `GET /api/hello` without requiring auth or actions:

```json
{
  "version": 1,
  "functions": {
    "hello": {
      "entry": "functions/hello.js",
      "methods": ["GET"],
      "mount": "/api/hello",
      "access": "anonymous",
      "actions": []
    }
  }
}
```

Names start with an ASCII letter and contain at most 64 letters, digits, `_` or `-`. Each mount is an absolute path with nonempty ASCII segments. Mounts claim the exact path and its descendants. They cannot overlap another function, static route, or reserved Kernel path. Methods are a nonempty list of unique `GET` and `POST` values. Access defaults to `authenticated`. `actions` defaults to an empty list. The entry must name a source file under `app/functions/`.

When a function uses Server Actions, its allowlist can name queries and mutations without external effects. It cannot name actions with `notify`, `signal`, `schedule`, `subscribe`, HTTP routes, grants, granted access, or a `then` target of an outgoing call. The build and Kernel check the declaration and artifact. The Kernel checks the allowlist, action schema, action access and actor bindings on every call.

## Run a function on a cron schedule

Declare the cron in the Capsule and publish the app. The same release enables Functions. If the cron target names Server Actions, the release enables Actions as well. In App settings under Modules, Functions lists the published cron jobs. Over MCP, call `modules_read` and read `modules.functions.current.crons`.

In `app/functions.json`, declare a Function with `"access": "scheduled"`. Leave out `mount` and `methods`. The Kernel calls this Function internally, so it has no HTTP route.

```json
{
  "version": 1,
  "functions": {
    "cleanup": {
      "entry": "functions/cleanup.ts",
      "access": "scheduled",
      "actions": ["sessions.expire"]
    }
  }
}
```

In `app/crons.json`, name the job, its cron expression and the Function to run.

```json
{
  "version": 1,
  "crons": {
    "weekday-cleanup": {
      "cron": "0 8 * * 1-5",
      "function": "cleanup"
    }
  }
}
```

This runs at 08:00 UTC from Monday through Friday. The five fields are minute, hour, day of month, month and day of week. Use numbers, `*`, lists such as `1,3,5`, ranges such as `1-5` and steps such as `*/15`. Sunday is `0` or `7`. If both day fields are restricted, either can match. An app can declare up to 32 jobs.

Export the same `handle(request, context)` as an HTTP Function. The Kernel passes an empty POST request, the job ID as `cron` and the scheduled UTC time in Unix milliseconds as `scheduledAt` in the query string. `context.actor.userId` is null. Set `"access": "scheduled"` on a Server Action reserved for scheduled Functions. Scheduled Functions may also call actions that already allow anonymous access.

```ts
export async function handle(_request, context) {
  await context.actions.call("sessions.expire", {});
  return new Response(null, { status: 204 });
}
```

The Kernel checks jobs at minute boundaries and attempts each job at most once per matching minute. It starts checking a release at the next minute boundary. Jobs in one app run in ID order. If the Kernel is down, the app is paused or its previous batch is still running, missed minutes are skipped. A failed attempt waits for the next matching minute. Cron attempts appear under `scheduled` in Usage and in `konae events` as `cron:<id>`.

`konae dev` pauses the timer. Run a job with `konae test schedule cron:weekday-cleanup` using the configured Functions runner. App exports carry `app/crons.json` and the active schedule in the published Server Artifact.

### Edit a cron with an agent

In the editor, the build agent reads `/workspace/crons.json` and `/workspace/functions.json`. It creates a missing file with `write` or changes an existing entry with `edit_json`. Changing just the time uses the JSON Pointer `/crons/weekday-cleanup/cron`. The agent reads the edit back and runs `validate_capsule`. Apply saves the source change. Deploy puts the new schedule into effect.

An outside agent can also edit the local `app/` files, or read and save the draft with `project_sources_read` and `project_sources_save`. Use the source revision from the read when saving over MCP. Read the saved sources back before publishing. [MCP tools and read-back](./mcp-tools.md#read-back-after-every-write)

## Write the handler

The builder bundles each entry and its dependencies into one ESM artifact. The Kernel requires the named export `handle` and refuses bundles with imports. Source imports must stay inside `app/`. External packages must be exact dependencies in `app/package.json`, with installed packages available to the builder.

For a no-framework function, `app/package.json` may be:

```json
{ "dependencies": {} }
```

The no-framework example needs no npm dependencies. The POST example uses only platform APIs. For the optional Hono version of `hello`, add this exact version to `app/package.json`:

```json
{ "dependencies": { "hono": "4.13.19" } }
```

The Hono version above matches the package in the runtime fixture. Choose an exact version supported by your app, install it, then commit the package lock. The bundle builder targets browser-platform ESM and rejects `node:` imports, host modules, URL imports, dynamic imports the builder cannot bundle, and packages not listed in dependencies. This is not Node.js or Deno. Do not assume `node:*`, unrestricted `fetch`, or APIs that the Web runtime does not provide. Hono works as an optional bundled package.

The context is supplied on every call. `context.actor.userId` is the account ID authenticated when the Kernel admitted the request, or `null`; it is always present. That actor stays fixed for the invocation. `context.actions.call(name, input)` checks the action's access and bindings against that actor. The handler cannot pass an actor ID, SQL, cookies, or a session credential to that operation. Only `Accept` and `Content-Type` request headers reach the handler. The app folder does not receive a generated `FunctionContext` type.

### Without a framework

Save one of these alternatives as `app/functions/hello.js`.

```js
export function handle(request) {
  const url = new URL(request.url);
  if (request.method === "GET" && url.pathname === "/api/hello") {
    return Response.json({ message: "Hello from Konae" });
  }
  return new Response("Not found", { status: 404 });
}
```

This handler matches the mount in the declaration. The Kernel passes the full mounted URL to it.

### With Hono

Hono is optional. Its adapter still exports the same `handle(request, context)` function:

```js
import { Hono } from "hono";

const app = new Hono();

app.get("/api/hello", (c) => {
  return c.json({ message: "Hello from Konae", userId: c.env.actor.userId });
});

export function handle(request, context) {
  return app.fetch(request, context);
}
```

The mount path remains in the Request URL. Match it in the handler or use Hono's `basePath` with the full mount. The Kernel does not strip the mount or trim a trailing slash. Hono's `app.fetch` receives the Request and Konae context as its environment argument. `c.env` exposes that context.

### With a Server Action

To add this recipe, turn on `auth` and `actions` alongside `functions` in `konae.json` or the app's module settings. Add this entry to the `functions` object in `app/functions.json`, then save the handler as `app/functions/orders.ts`.

```json
"orders": {
  "entry": "functions/orders.ts",
  "mount": "/api/orders",
  "methods": ["POST"],
  "access": "authenticated",
  "actions": ["orders.create"]
}
```

Declare `orders.create` in `actions.json` as an authenticated mutation with one string input named `title`, a maximum length of 120, and no external effects. [Add a Server Action](./guides/add-a-server-action.md) shows the `actions.json` bindings and SQL. The action returns `{ changes, lastInsertId }`.

```ts
type CreateOrderResult = { changes: number; lastInsertId: number | null };
type OrderContext = {
  actor: { userId: string | null };
  actions: {
    call(name: "orders.create", input: { title: string }): Promise<CreateOrderResult>;
  };
};

export async function handle(request: Request, context: OrderContext): Promise<Response> {
  if (request.method !== "POST") return Response.json({ error: "method_not_allowed" }, { status: 405 });

  let input: unknown;
  try {
    input = await request.json();
  } catch {
    return Response.json({ error: "invalid_json" }, { status: 400 });
  }
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    return Response.json({ error: "invalid_input" }, { status: 400 });
  }
  const title = (input as { title?: unknown }).title;
  if (typeof title !== "string" || title.trim().length === 0 || title.length > 120) {
    return Response.json({ error: "invalid_title" }, { status: 400 });
  }

  const result = await context.actions.call("orders.create", { title: title.trim() });
  return Response.json({ orderId: result.lastInsertId, changes: result.changes }, { status: 201 });
}
```

`OrderContext` is a narrow type written for this handler, not a type Konae generates. Keep its action name, input and result aligned with `actions.json` and the action contract. A POST with actions requires the caller to send `Idempotency-Key` with 1 to 128 visible ASCII bytes. The Kernel does not expose that header to the handler. Let action-call rejections reach the Kernel so it can return its canonical refusal and roll back pending changes. Do not turn a refusal into a success response or retry the same intent with a new key.

## Build and run

The examples name `app/functions/hello.js` and `app/functions/orders.ts`. Run the installed CLI from the app directory with `konae build --dir .` and `konae dev --dir .`. The no-framework example needs no package dependencies. The Hono example has an exact dependency in `app/package.json`. The normal build produces public `dist/` files and a separate `server/` directory with bundled functions. The Kernel serves only `dist/` to browsers. `konae dev` builds the same server artifact and runs it through the configured runner.

Run installation commands from the folder containing `konae.json`. Use `npm ci --prefix app` when `app/package-lock.json` exists, or `npm install --prefix app` otherwise. If this folder also has a root `package-lock.json` for its build tools, run `npm ci` here too.

```sh
konae build --dir .
KONAE_FUNCTIONS_RUNNER=/absolute/path/to/konae-functions-runner \
KONAE_FUNCTIONS_DEVELOPMENT=1 konae dev --dir .
```

From a Konae checkout, use its CLI script and Kernel binary as a pair. Build the Kernel from that checkout before this command:

```sh
KONAE_REPO=/path/to/konae
APP_DIR=/path/to/my-app
bun "$KONAE_REPO/scripts/konae.ts" build --dir "$APP_DIR"
KONAE_KERNEL_BIN="$KONAE_REPO/kernel-rs/target/debug/kernel" \
KONAE_FUNCTIONS_RUNNER=/absolute/path/to/konae-functions-runner \
KONAE_FUNCTIONS_DEVELOPMENT=1 \
bun "$KONAE_REPO/scripts/konae.ts" dev --dir "$APP_DIR"
```

The runner and Kernel must come from the same build. Local development mode uses RSS polling and does not apply Linux cgroup, identity, Landlock or seccomp isolation. On a deployment, the operator configures `KONAE_FUNCTIONS_RUNNER` and a writable delegated cgroup v2 subtree through `KONAE_FUNCTIONS_CGROUP`, with the separate worker UID/GID and Linux namespaces described in [Operate an installation](./operate.md#typescript-function-workers). Never use development mode as the deployment boundary. The editor preview does not execute functions.

Once `konae dev` reports the app URL, set `APP_URL` to that exact printed URL, including its local hostname, then call the mount:

```sh
APP_URL='http://<the-exact-host-printed-by-konae-dev>'
curl -i "$APP_URL/api/hello"
```

Use the app's assigned host. A request to `127.0.0.1` with no matching `Host` header may select no app. This public example needs no session. Authenticated functions require a valid app session from a same-origin client; functions do not accept Konae owner credentials or app API keys.

## Identity, actions and request rules

The Kernel authenticates the request before it starts the function. Authenticated functions require an app user session. Anonymous functions may run without one on a public Deployment. The module must be on, the runner must be available, the method and mount must match, and the caller must pass the deployment's request protections. An app API key is refused for Functions. A disabled module or a path that matches no function does not create a worker. The Kernel starts a worker on the first admitted request and retires it after its idle timeout.

The handler sees `context.actor.userId`, with `null` for an anonymous request. The Kernel checks each action's access and bindings against the actor fixed at request admission. Keep row access rules in `actions.json`; do not trust a user ID from the request body. A `GET` function may call queries only. A `POST` with allowed actions requires an `Idempotency-Key` of 1 to 128 visible ASCII bytes. Reuse the same key for retries of the same method, path, permitted headers and body.

The Kernel runs a POST action invocation in a SQLite transaction. It commits action changes and a request receipt together when the handler returns a successful response. A handler error, action failure, invalid or oversized response, or response with status 400 or higher rolls back pending action changes. A timeout before commit also rolls them back, but a timeout after commit may leave the result uncertain. In that case the Kernel returns `409 mutation_outcome_unknown` and tells the caller to repeat the exact request with the same `Idempotency-Key`. If the commit succeeded, that retry returns `409 idempotency_replay` without running the handler again or returning its old response. Read the current result through an authorized function request or Server Action. The read applies the app's current row rules. Never assume a timeout means rollback, and never use a new key to retry the same intent. A successful response without actions has no data transaction. If the same key names a different request, the Kernel returns `409 idempotency_conflict`.

The Kernel authenticates each retry before looking up its receipt. An identical retry returns `409 idempotency_replay`, even when the original response body remains in an older receipt. New receipts keep the request fingerprint and a fixed marker instead of the response body. The existing receipt table stays unchanged, so an app export can still open on this Kernel. A caller must read current state through an authorized request.

For POST functions with actions, consume the complete request body before the first `context.actions.call`. If the handler calls an action early, the invocation fails even if it catches the rejected Promise. Each request evaluates the bundle in a fresh V8 isolate, even when the Kernel reuses the worker process. Store durable state through Server Actions. `Request.signal` lets a handler cooperatively stop after cancellation. If JavaScript is still blocked, the runner terminates the isolate. The supervisor can kill a worker that fails to respond.

A bundle may export `shutdown(signal)` to release resources when the worker retires. The runner calls it in the last completed isolate, if one is still available. The hook has a short deadline and may not run after a crash, memory exhaustion or a blocked loop. It does not roll back a transaction that already committed.

## Separate limits

The HTTP request body limit is 100,000,000 bytes (decimal). The Kernel counts bytes received, including chunked bodies without `Content-Length`. It forwards the body in chunks up to 64 KiB and keeps at most 8 MiB of pending request chunks across active functions. The limit does not reserve a 100 MB buffer or spool the body to disk. A full pending-body budget fails with `503`; an oversized body fails with `413`. The deadline covers upload, queueing, initialization, IPC and handler work.

Other limits remain independent. The default worker limits are 64 MiB heap and 256 MiB RSS. JSON parsing can allocate the full request and exceed those limits. Each serialized action operation is capped at 1 MiB after JSON escaping, with at most 64 operations. A query returns at most 1,000 rows and 2 MiB. Function response bodies and their serialized validation envelope have a 1 MiB cap; response headers have a 16 KiB cap. New receipts do not retain response bodies. Uploaded files use their own limits. See [ADR-0019](./adr/0019-functions-stream-request-bodies-with-bounded-ipc.md) and [Limits](./limits.md).

Function response bodies must be valid UTF-8. Use text or JSON for responses and the Files module to serve binary files. The runner refuses bytes that cannot be decoded as UTF-8.

An app may declare at most 32 functions. Each bundle must be 1 to 1,048,576 UTF-8 bytes, with 8,388,608 bytes total across bundles. `server/manifest.json` is capped at 262,144 UTF-8 bytes, including hashes.

## Export and move an app

The export carries `app/functions.json`, source files and package metadata, the published `server/` bundles, app data and app accounts. The `server/` bundle is separate from public `dist/`. Import restores the exported app data and published artifact, then the destination Kernel runs the bundle with its own matching runner. Session tokens and installation secrets do not travel. Build or publish the app again after changing its function source. The source tree, published bundle and current deployment can therefore represent different revisions until a successful publish.

Read [Take the app elsewhere](./guides/take-the-app-elsewhere.md) for the full export and import flow. Functions require the destination operator to configure a compatible runner before requests can execute.

## Compatibility and failure boundaries

The app author chooses exact npm dependency versions. The builder bundles installed packages into ESM. The runtime does not resolve npm packages at request time. Packages that require Node builtins, native addons, host modules, network access, or unsupported Web APIs will not work just because npm can install them. Hono is an optional framework dependency, not a runtime requirement.

The Kernel returns a refusal if Functions are off, a dependency module is off, the runner is unavailable, the IPC pair does not match, the handler calls an undeclared action, the action is refused, the request exceeds a limit, or the execution deadline expires. `Request.signal` and `shutdown` let application code respond to some cancellations, but neither grants work after the request ends. Functions remain experimental; see [the architecture record](./adr/0018-typescript-functions-load-on-demand-and-use-kernel-services.md) for the implementation boundary and evidence status.
