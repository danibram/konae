# Google sign-in for app users and an S3 file store

Proposal written on November 9, 2026. It changes no accepted ADR and implements nothing. Part 2 needs a new ADR before code.

## Why

A small practice app moving onto Littlebox needs two things the Kernel does not offer. Its staff sign in with Google today and do not want another password. Its patient photos and attachments outgrow the 20 MiB a Deployment may hold. The app is one Deployment per practice; a second practice gets its own app.

## How each part passes "How to decide"

AGENTS.md admits a capability only when the Validation Gate and the Kernel refuse it with the same words and a package carries the app's accounts, data and rules. Anything else is operator work on the installation.

| Part | App decision, carried in the package | Installation work, never in the package |
| --- | --- | --- |
| Google sign-in | Whether this Deployment accepts Google, next to its Signup Policy. The accounts, which Google reaches by email. | Nothing. The app's own Google client is a pair of project secrets the owner sets on each installation, and secrets never travel. |
| S3 file store | Nothing new. Files keep their IDs, visibility and uploader, and a package still carries their bytes. | Where the Kernel keeps file bytes, and the quotas it allows. |

A per-app S3 connector, where the app holds its own bucket credentials, fails the rule three ways. The credentials do not travel, a package would not carry the files, and the app policy's `connect-src 'self'` (`kernel-rs/src/security.rs`) stops the page from reaching the bucket. This plan does not propose it.

## Part 1: Google sign-in for app users

### Result

On a Deployment whose owner turned Google on, the sign-in page shows "Continue with Google" next to email and password. A person who signs in with Google gets the same app account, the same `actor.id` and the same role as with a password. Signup Policy decides whether that person may have an account at all.

### Decisions

1. **One client per app, set by its owner.** Revised on November 9, 2026: the installation has no Google client. The app's owner makes a Web application OAuth client in their own Google Cloud console and sets it as two project secrets, `google_client_id` and `google_client_secret`, with `secret_set` and `use: "google"`, or in Auth > Users. Those names take only that use, the values are never returned, and no route or call reads them. `sign_in_methods_read` reports whether each is set (`googleClient { clientIdSet, clientSecretSet }`) and the redirect URIs to register. `sign_in_methods_set` refuses `google` while either is unset with "Set this app's Google client ID and client secret before turning Google on.", and `secret_delete` refuses either while Google is on, so only the owner turns Google off. A dedicated tool was not added: `secret_set` already holds values the Kernel never returns, and the editor fields are what a person who does not write code meets.
2. **The callback on the app's own host.** With a client per app, the owner registers `https://<app host>/_littlebox/auth/google/callback`, plus the same path on a verified custom domain (ADR-0015). Google matches redirect URIs exactly, scheme, case and trailing slash included. The flow runs `/_littlebox/auth/google/start` → Google → `/_littlebox/auth/google/callback` on the same host. The callback takes the flow once, only on the Deployment, origin and browser cookie it started with, exchanges the code with PKCE and opens the app's own session. The Kernel-host callback, the finish step and its code store are gone, and Google sign-in no longer needs `LITTLEBOX_PUBLIC_ORIGIN` to be anything in particular.
3. **Keys are fetched, unlike Clerk's.** `clerk.rs` reads its JWKS from an operator file by design. Google rotates its keys, so the Kernel fetches `https://www.googleapis.com/oauth2/v3/certs` from that fixed URL only, caches it by the response's `Cache-Control`, and refetches once on an unknown `kid`. It never follows a URL taken from a token. The ID token must name Google as issuer, this client as audience, an unexpired time, the `nonce` the start step set, and `email_verified: true`.
4. **No new kind of account and no identity table.** `app_users.password_hash` stays `NOT NULL` (`kernel-rs/src/auth.rs`); an account created through Google gets an unusable random hash, so an older Kernel that imports the package treats it as a password account whose owner recovers it. The `app_user_identities` table of the first build is gone, so packages carry nothing new; a package written by that build still imports, because `package.rs` checks only `app_users`.
5. **Accounts are matched by verified email.** Revised on November 9, 2026, replacing "no join by matching email" for app users. A Google sign-in reaches the account that holds the email, compared without case as `app_users.email` is, whether it was made with a password or with Google. Google's email is proof only when Google is the authority for the address: it ends in `@gmail.com`, or the token's `hd` claim equals its domain (Google Workspace). Google's docs warn that the `email` claim "may not be unique to this account and could change over time", so any other address is refused with "This Google account uses an email address Google does not manage, so it cannot sign in here. Sign in with email and password." The check is one named function, `google_vouches_for` in `kernel-rs/src/google.rs`. A suspended account is refused. A pending account keeps waiting: a signup waiting for its confirmation link answers "Confirm your email", and an invited one accepts through the invitation page, which records the acceptance. Matching by email also makes portability independent of the Google `sub`. Google's discovery document lists `subject_types_supported: ["public"]`, so `sub` is the same for every client anyway.
6. **Signup Policy still decides.** Closed admits only linked accounts. Invitation only admits a new account only through an invitation link, and the Google email must equal the invited email; accepting through Google activates the account as the password path does in `invitations.rs`. Allowlist runs its email check on the Google email. Open creates the account. The Kernel rechecks the policy in the same transaction that creates or links, as it does today. The owner's own app account keeps its current creation rule.
7. **A setting of the Deployment, carried in the package.** `signInMethods: ["password"] | ["password", "google"]` sits on the Deployment record and in the package manifest next to `signup_policy` (`kernel-rs/src/package.rs`). The owner changes it in Auth > Users or with a `sign_in_methods_set` MCP tool that needs `manage_users`, read back with `sign_in_methods_read`. Password cannot be turned off in this version, so every account always has a way back in. `initialSignInMethods` on a first release comes only from an export, so it is kept like a package's setting rather than refused.
8. **Moving to another installation.** Secrets never travel. The package carries `signInMethods` and the accounts. On the destination the Deployment keeps the setting, its sign-in page offers only email and password, and `sign_in_methods_read` returns a notice, until the owner sets the two secrets there and registers the destination's redirect URI in the Google client. Then the same person reaches the same account by email. A person whose account came from Google can also set a password through the existing recovery email (ADR-0012). That keeps job 4.
9. **SDK.** `miniapp.auth.signInWithGoogle()` navigates the top window to the start path; it is a navigation, so `connect-src` does not apply.  The contract in `src/sdk/contract.ts` gains the call and its refusals, and `docs/sdk.md` and the `sdk` topic are rendered from it.
10. **Named divergence.** The editor preview has no Google sign-in. Add it to the AGENTS.md list and to `kernel-rs/src/guide.rs`.

### Work

| Piece | Where |
| --- | --- |
| Start, callback on the app host, flow store, JWKS cache, `google_vouches_for` | `kernel-rs/src/google.rs`, routes in `kernel-rs/src/main.rs` |
| Sign-in by verified email, invitation answer | `kernel-rs/src/auth.rs` |
| `use: "google"` secrets, delete refusal | `kernel-rs/src/http.rs` |
| Policy check on create and on invitation acceptance | `kernel-rs/src/accounts.rs`, `kernel-rs/src/invitations.rs` |
| `signInMethods` on the record, manifest and import | `kernel-rs/src/package.rs`, `kernel-rs/src/migration.rs`, deployment record |
| MCP tools and the `rules` read-back | `kernel-rs/src/mcp.rs`, `kernel-rs/src/guide.rs` |
| Sign-in page button, invitation page "Accept with Google" | Kernel sign-in HTML, `src/sdk/` |
| Auth > Users card: redirect URIs with copy, client fields, switch | `src/components/AppUsersView.tsx` |

### Tests

- A forged, expired, wrong-audience (another app's client), wrong-nonce or unverified-email token is refused.
- A flow works once, only on the Deployment, origin and browser it started in, and not after it expires; a host whose callback the owner was not shown starts no flow, and the redirect URI sent equals the one shown.
- Closed, Invitation only, Allowlist and Open each admit and refuse as the table above says, with the same words as the password path.
- A password account is signed in by Google with the same email; a non-authoritative email is refused; a Workspace address whose `hd` names its domain is accepted.
- An identity database moved to another app with a different client, as a package moves it, signs the same person in with a different `sub`.
- `sign_in_methods_set` refuses google until both secrets are set; `secret_delete` refuses either while Google is on; `sign_in_methods_read` never returns a value.
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

- Other providers (Microsoft, Apple). The `issuer` column keeps the door open; each one needs its own key and claim rules.
- Turning password sign-in off for a Deployment.
- Files that every member of a Deployment may delete, not only the uploader.
