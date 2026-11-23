# Node compatibility build spike

Run from the Konae checkout with Bun, npm and Google Chrome installed. Vite serves a local test page. Chrome runs the editor's esbuild-wasm build against live esm.sh. The CLI uses the installed packages. The two in-memory patches only insert the compatibility resolver into the existing builders. They leave the checkout's production files alone.

```sh
bun install --frozen-lockfile
npm ci --prefix spikes/node-compat --ignore-scripts
bun spikes/node-compat/build.mjs
```

Expect 12 matching HTTP-shaped results in Chrome and 14 matching refusals. The build validates each Server Artifact and writes the two positive app folders under `/tmp/konae-functions-node-compat-design-spike`. It also writes a separate refused app folder. Pass a scratch directory as the first argument to use another location. To use a different Chrome channel, set `KONAE_SPIKE_CHROME`.

Build the checkout Kernel and runner, then run the two bundles through HTTP.

```sh
(cd kernel-rs && cargo build -p kernel)
cargo build --release --manifest-path kernel-rs/functions-runner/Cargo.toml
bun ./spikes/node-compat/http.mjs
```

The harness starts `konae dev` with the spike resolver and opens the editor artifact on another Kernel without a rebuild. Each lane expects seven HTTP 200 responses and five HTTP 500 `handler_error` responses on the stock deno_core runner. Run with the scratch directory and a second argument of `full` to require every route to return its expected HTTP 200 body. Set `KONAE_KERNEL_BIN` and `KONAE_FUNCTIONS_RUNNER` for a paired build elsewhere.

`cases.mjs` owns the expected responses. The private JWKs in `test-jwks.json` are public test data for the JWT round trip. The build harness imports both production builders directly. The architecture note records the module decisions and their remaining runner work.
