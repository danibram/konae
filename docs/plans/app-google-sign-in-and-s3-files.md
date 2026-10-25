# Google sign-in for app users and an S3 file store

Proposal written on November 9, 2026. It changes no accepted ADR and implements nothing. Part 2 needs a new ADR before code.

## Why

A small practice app moving onto Littlebox needs two things the Kernel does not offer. Its staff sign in with Google today and do not want another password. Its patient photos and attachments outgrow the 20 MiB a Deployment may hold. The app is one Deployment per practice; a second practice gets its own app.

## How each part passes "How to decide"

AGENTS.md admits a capability only when the Validation Gate and the Kernel refuse it with the same words and a package carries the app's accounts, data and rules. Anything else is operator work on the installation.

| Part | App decision, carried in the package | Installation work, never in the package |
| --- | --- | --- |
| Google sign-in | Whether this Deployment accepts Google, next to its Signup Policy. The link between an app account and a Google subject. | The Google OAuth client: ID, secret and the one redirect URI. |
| S3 file store | Nothing new. Files keep their IDs, visibility and uploader, and a package still carries their bytes. | Where the Kernel keeps file bytes, and the quotas it allows. |

A per-app S3 connector, where the app holds its own bucket credentials, fails the rule three ways. The credentials do not travel, a package would not carry the files, and the app policy's `connect-src 'self'` (`kernel-rs/src/security.rs`) stops the page from reaching the bucket. This plan does not propose it.

## Part 1: Google sign-in for app users

### Result

On a Deployment whose owner turned Google on, the sign-in page shows "Continue with Google" next to email and password. A person who signs in with Google gets the same app account, the same `actor.id` and the same role as with a password. Signup Policy decides whether that person may have an account at all.

### Decisions

1. **One client per installation.** The operator registers one Google OAuth client and sets `LITTLEBOX_GOOGLE_CLIENT_ID`, `LITTLEBOX_GOOGLE_CLIENT_SECRET` and the Kernel's public origin. Without them the setting is hidden and `sign_in_methods_set` refuses `google` with "This installation has no Google sign-in." A per-app client is deferred.
2. **One callback on the Kernel's own host.** Google accepts only redirect URIs registered exactly, and each Deployment answers on its generated host and possibly a custom domain (ADR-0015) with its own cookies. So the flow runs `app host /_littlebox/auth/google/start` → Google → `kernel host /_littlebox/auth/google/callback` → `app host /_littlebox/auth/google/finish?code=…`. The callback never sets an app cookie. It stores a single-use code bound to the Deployment, the app origin and the `state`, valid for 60 seconds, and the app host exchanges it and opens its own session. Reuse the code-exchange shape of `kernel-rs/src/clerk.rs` (`exchange`) and `kernel-rs/src/oauth.rs`. Confirm Google's exact-match rule against its current OAuth documentation before building.
3. **Keys are fetched, unlike Clerk's.** `clerk.rs` reads its JWKS from an operator file by design. Google rotates its keys, so the Kernel fetches `https://www.googleapis.com/oauth2/v3/certs` from that fixed URL only, caches it by the response's `Cache-Control`, and refetches once on an unknown `kid`. It never follows a URL taken from a token. The ID token must name Google as issuer, this client as audience, an unexpired time, the `nonce` the start step set, and `email_verified: true`.
4. **Linked identities, not a new kind of account.** `identity.sqlite` gains `app_user_identities(user_id, issuer, subject, email, linked_at)` with `(issuer, subject)` unique. `app_users.password_hash` stays `NOT NULL` (`kernel-rs/src/auth.rs`); an account created through Google gets an unusable random hash, so an older Kernel that imports the package treats it as a password account whose owner recovers it. The table is created by the existing `migrate` in `auth.rs`, and `package.rs` keeps checking only `app_users`, so old packages still import.
5. **No join by matching email.** This repeats the creator rule in `signup-and-app-invitations.md`. A first Google sign-in whose email belongs to an existing account is refused with "An account with this email exists. Sign in with your password, then connect Google." A signed-in person connects Google from their account, and only then is the subject linked.
6. **Signup Policy still decides.** Closed admits only linked accounts. Invitation only admits a new account only through an invitation link, and the Google email must equal the invited email; accepting through Google activates the account as the password path does in `invitations.rs`. Allowlist runs its email check on the Google email. Open creates the account. The Kernel rechecks the policy in the same transaction that creates or links, as it does today. The owner's own app account keeps its current creation rule.
7. **A setting of the Deployment, carried in the package.** `signInMethods: ["password"] | ["password", "google"]` sits on the Deployment record and in the package manifest next to `signup_policy` (`kernel-rs/src/package.rs`). The owner changes it in Auth > Users or with a `sign_in_methods_set` MCP tool that needs `manage_users`, read back with `sign_in_methods_read`. Password cannot be turned off in this version, so every account always has a way back in.
8. **Moving to an installation without Google.** The package carries the links. On import, a Deployment that asks for Google on an installation without a client keeps the setting, shows only email and password, and says so to the owner. A person whose account came from Google sets a password through the existing recovery email (ADR-0012). That keeps job 4.
9. **SDK.** `miniapp.auth.signInWithGoogle()` navigates the top window to the start path; it is a navigation, so `connect-src` does not apply. `miniapp.auth.session()` reports `methods` for the signed-in account. The contract in `src/sdk/contract.ts` gains the call and its refusals, and `docs/sdk.md` and the `sdk` topic are rendered from it.
10. **Named divergence.** The editor preview has no Google sign-in. Add it to the AGENTS.md list and to `kernel-rs/src/guide.rs`.

### Work

| Piece | Where |
| --- | --- |
| Start, callback, finish, code store, JWKS cache | new `kernel-rs/src/google.rs`, routes in `kernel-rs/src/main.rs` |
| `app_user_identities`, link, unlink, sign-in by subject | `kernel-rs/src/auth.rs` |
| Policy check on create and on invitation acceptance | `kernel-rs/src/accounts.rs`, `kernel-rs/src/invitations.rs` |
| `signInMethods` on the record, manifest and import | `kernel-rs/src/package.rs`, `kernel-rs/src/migration.rs`, deployment record |
| MCP tools and the `rules` read-back | `kernel-rs/src/mcp.rs`, `kernel-rs/src/guide.rs` |
| Sign-in page button, account page "Connect Google" | Kernel sign-in HTML, `src/sdk/` |
| Auth > Users toggle | editor Auth panel |

### Tests

- A forged, expired, wrong-audience, wrong-nonce or unverified-email token is refused.
- A finish code works once, only on the Deployment and origin it was issued for, and not after 60 seconds.
- Closed, Invitation only, Allowlist and Open each admit and refuse as the table above says, with the same words as the password path.
- A Google email that matches an existing account is refused until that account connects Google.
- Package export and import keep the links; an import onto an installation without a client shows the notice and recovery sets a password.
- A package written before this change imports unchanged.

### Cheaper alternative

The mail queue already sends verified email, so a "send me a sign-in link" option would remove passwords without a third party or a new installation setting. It does not give the one-click Google sign-in the practice asked for, so this plan keeps Google and leaves the link as a later option.

## Part 2: S3 as the installation's file store

### Result

An operator points the Kernel at an S3-compatible bucket. Deployment files go there instead of the Deployment folder, and the operator raises the file quotas. Apps, the SDK and packages see no difference.

### Decisions

1. **A storage trait with two implementations.** A `FileStore` trait with `put`, `get` (ranged), `delete` and `list` for one Deployment. `LocalFolder` is today's `deployments/<id>/files/<id>.blob`. `S3Store` takes endpoint, region, bucket, prefix and keys from `LITTLEBOX_FILES_S3_*` and stores `<prefix>/<deployment id>/<file id>`. It uses only the operations `docs/research/provider-portability.md` lists for R2, Scaleway and Hetzner. The file index stays in the Deployment folder, so the Kernel answers list, visibility and quota without calling S3.
2. **Quotas become operator settings.** The 20 MiB quota and the 2 MiB per-file cap are constants today (`kernel-rs/src/files.rs`, `kernel-rs/src/main.rs`, `src/sdk/contract.ts`). They become `LITTLEBOX_FILES_QUOTA_BYTES` and `LITTLEBOX_FILE_MAX_BYTES`, with today's values as defaults. Without this step S3 changes nothing for the app that asked for it. The refusal text names the configured number, and the SDK contract reads the limits from the Kernel instead of stating fixed numbers.
3. **Bytes still go through the Kernel.** Uploads and downloads keep the existing 64 KiB block transfers and SHA-256 check, and `GET /_littlebox/files/<id>` streams from the store. No presigned URL reaches the browser; the page policy would refuse it anyway. A larger per-file cap is a transfer-time question. Measure a 25 MiB upload before choosing the default for S3.
4. **A Deployment is no longer one folder.** CONTEXT.md says everything a Deployment is lives in its folder. With S3 the bytes live in the bucket and the folder holds the index. The new ADR records that and states what each operation does:
   - Package export (`package.rs`, `write_files_tar`) streams each blob from the store into the tar. Import writes into whatever store the destination uses. Packages do not change.
   - A copy of the installation (`copies.rs`) copies the index and either copies the objects under a new prefix or records that the copy shares the bucket. Pick one in the ADR; sharing makes a restore able to resurrect deleted files.
   - `restore_initial_files` (`main.rs`) and migration write through the trait.
   - Deleting a file or a Deployment deletes its objects after the index change commits, and a reconcile pass removes objects the index no longer names.
5. **Moving between stores.** `littlebox-kernel files-migrate --to s3` copies every Deployment's blobs, verifies each SHA-256, then switches the setting. The reverse direction exists for an operator leaving S3.

### Work

| Piece | Where |
| --- | --- |
| `FileStore`, `LocalFolder`, `S3Store` | new `kernel-rs/src/file_store.rs` |
| Upload, read, delete through the trait | `kernel-rs/src/files.rs`, `kernel-rs/src/http.rs` |
| Quota and per-file cap from settings | `kernel-rs/src/files.rs`, `kernel-rs/src/main.rs`, `src/sdk/contract.ts` |
| Package, copy, restore, migration | `kernel-rs/src/package.rs`, `kernel-rs/src/copies.rs`, `kernel-rs/src/migration.rs`, `kernel-rs/src/main.rs` |
| `files-migrate` and reconcile | Kernel command line |
| ADR and CONTEXT.md "Deployment" | `docs/adr/`, `CONTEXT.md` |

### Tests

- The file tests in `files.rs` run against both stores, with a local S3-compatible server for `S3Store`.
- A package exported from an S3 installation imports into a local-folder one with the same bytes and hashes, and the reverse.
- A raised quota admits a file the default refuses, and the refusal names the configured limit.
- Contract tests against R2, Scaleway and Hetzner, as the research note asks, before calling S3 supported.

## Order

1. Part 2, decisions 1 and 2 only: the trait with `LocalFolder` and the quota settings. This alone unblocks the practice app on a large disk.
2. The ADR, then `S3Store`, package and copy support, `files-migrate`.
3. Part 1, Google sign-in.

## Deferred

- A Google client per Deployment, for an owner who wants the consent screen to show the app's own name.
- Other providers (Microsoft, Apple). The `issuer` column keeps the door open; each one needs its own key and claim rules.
- Turning password sign-in off for a Deployment.
- Files that every member of a Deployment may delete, not only the uploader.
