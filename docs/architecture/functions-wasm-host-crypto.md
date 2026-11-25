# Web Crypto in the StarlingMonkey runner

A function uses the same Web Crypto calls on either runner. The app folder carries the function's JavaScript. The destination installation prepares the bundle with its own runner and helper.

## Before the adapter

I built the 18-route Node compatibility spike with `bun spikes/node-compat/build.mjs <dir>` on an Apple M3 Pro running macOS 26.6.1. I used the esbuild CLI bundle for `/api/jose`, the preparer and runner from `a8b9c977`, and an instrumented copy of the bundle. The probe wrapped JOSE's Web Crypto calls, two base64url helpers, and JSON parse and stringify with `performance.now()`. It sent one IPC/3 Invoke to a fresh worker with the deadline left to the driver, so the call could finish. The worker returned HTTP-shaped status 200 with both verified claims and rejected both changed signatures.

| Guest operation | Calls | Elapsed ms |
| --- | ---: | ---: |
| RSA private-key import | 1 | 1195.00 |
| RSA public-key import | 1 | 69.15 |
| RSA sign | 1 | 21.99 |
| RSA verify | 2 | 1.41 |
| ECDSA private and public key import | 2 | 9.36 |
| ECDSA sign | 1 | 4.65 |
| ECDSA verify | 2 | 4.82 |
| JOSE base64url | 16 | 0.20 |
| JOSE JSON | 10 | 0.11 |

The guest took 1310.07 ms. The IPC round trip took 1324.82 ms, and the child's user plus system CPU was 1236.22 ms including startup and shutdown. The probe measures elapsed time by operation. The signature rows include the digest and signature math. The route calls no standalone digest, and the base64url rows cover the JOSE helpers outside key import. Under `konae dev`, the unchanged bundle returned 504 `deadline_exceeded` in 1006.97 ms.

I repeated the probe with the host adapter. Guest time fell to 86.75 ms, the round trip to 104.75 ms, and child CPU to 97.85 ms. RSA private-key import took 60.29 ms. That call includes OpenSSL's consistency check on the imported private key.

## Host interface

The StarlingMonkey guest installs the Web Crypto facade before evaluating the app module. It normalizes Web API arguments and calls six typed imports in `konae:worker/crypto`. The runner uses statically linked OpenSSL 3.6.3 through the Rust `openssl` crate for hashing, key parsing and signature operations. The RSA-PSS implementation honors the caller's salt length, including zero. The adapter converts ECDSA signatures between OpenSSL DER and the Web Crypto 64-byte P-256 format. HMAC verification compares equal-length byte strings in constant time. The entropy import fills at most 65,536 bytes per call and backs both `getRandomValues` and `randomUUID`. [OpenSSL signature API](https://docs.rs/openssl/0.10.82/openssl/sign/index.html)

| Algorithm | Operations and key formats |
| --- | --- |
| SHA-1, SHA-256, SHA-384 and SHA-512 | Digest |
| HMAC | Sign and verify. Import and export raw and JWK secret keys. |
| ECDSA P-256 | Sign and verify. Import and export raw and SPKI public keys, PKCS8 private keys, and JWK public and private keys. |
| RSASSA-PKCS1-v1_5 and RSA-PSS | Sign and verify. Import and export SPKI public keys, PKCS8 private keys, and JWK public and private keys. |

Each restored Wasmtime Store owns a table of up to 64 keys. The guest's `CryptoKey` holds metadata and maps privately to a numeric handle. Rust checks the key's algorithm, usages and extractability at the call boundary. Native keys are released when the invocation finishes or the Store drops. The host returns key bytes only for an explicit export of an extractable key. Crypto requires a live request. Build-time use triggers the existing initialization refusal even if the app catches the exception. The host checks cancellation and deadline on both sides of a native call. The parent can retire the worker if a native call outlasts cancellation.

The Wasm host bounds key input at 16 KiB, cryptographic input at 16 MiB per operation and RSA modulus size at 8192 bits. It accepts two-prime RSA keys and at most 32-bit public exponents. These are adapter limits. The worker's normal heap, RSS and deadline also apply.

The Kernel checks the six function signatures when the component imports crypto. The shared fixtures exercise a wrong signature and an extra export with the existing `function.wasm-import` code and message. Older components without the interface still pass validation. The preparer's fingerprint includes the adapter, so preparation uses a new cache entry for the same bundle. The app artifact and IPC/3 formats stay the same.

The deno_core facade exposes the same methods over its existing crypto extension. Its bootstrap registers the extension's DOMException classes so the runners can agree on the names users can catch. Both runner suites use the same digest, HMAC, ECDSA and RSA vectors and key-format cases. The signature vectors use the public test keys in the Node compatibility spike. RSA-PSS covers zero and nonzero salt lengths and a rejection for the wrong salt length.

## HTTP proof

I started the checkout's Kernel with `bun scripts/konae.ts dev` on fresh copies of the CLI app folder. I selected each runner through `KONAE_FUNCTIONS_RUNNER` and used development polling on macOS. For StarlingMonkey I also set `KONAE_FUNCTIONS_RUNNER_KIND=wasm`, `KONAE_FUNCTIONS_PREPARER` and `KONAE_FUNCTIONS_WASM_CACHE`. The Kernel used its default 1000 ms deadline. On each runner all 18 positive routes returned 200 with the JSON expected by `spikes/node-compat/cases.mjs`. This includes the Web Crypto, timers and structured clone checks in `GET /api/web`. `GET /api/jose` returned 200 with `{"ES256":"konae","RS256":"konae"}`.

For cold timing I sent `GET /api/jose` as the first function request after health on five separate `konae dev` starts. The Wasm cache held the prepared components by the time health answered. For warm timing I sent another 20 calls on one of those starts before calling any other function. The client timed each loopback GET through response body read. These runs share the Mac with other jobs and took place at different times. The samples describe the observed deadline margin.

| Runner | Cold min / median / max, ms | Warm min / median / max, ms |
| --- | ---: | ---: |
| StarlingMonkey with host crypto | 448.54 / 475.09 / 614.19 | 41.05 / 44.72 / 59.92 |
| deno_core | 126.40 / 196.37 / 249.41 | 21.59 / 35.14 / 125.30 |
