# Owner sign-in with Clerk

Konae can let owners sign in through Clerk with Google instead of with an email and a password. It is optional. Without it, owners sign in with the accounts, verification and invitation codes [Operate an installation](./operate.md#creator-admission) describes. `kernel-rs/konae/src/clerk.rs` holds the code. The `kernel` binary has no owners and reads none of this.

Setting any `KONAE_CLERK_*` variable turns Clerk sign-in on for the whole installation. Konae then refuses owner sign-in and signup with a password, and the owner sign-in page at `/_konae/owner/login` sends the browser to the editor's `/platform-access`, which holds Clerk's sign-in. A missing or invalid required setting stops sign-in with a configuration error and never falls back to passwords. App users keep their own accounts in each app.

## Configure it

Create a Clerk application with Google as its only sign-in and signup method. Clerk's session token does not say which provider a person used, so Konae cannot check that from the token.

Copy the instance's JWKS from its Frontend API URL with `/.well-known/jwks.json` appended, and store the JSON in a file only the operator can change. Konae reads that file and never fetches a key from a URL a token names.

| Variable | Value |
| --- | --- |
| `KONAE_CLERK_ISSUER` | The instance's exact HTTPS origin, without a trailing slash |
| `KONAE_CLERK_JWKS_FILE` | Path to the JWKS file, at most 64 KiB, with one to sixteen RS256 signing keys |
| `KONAE_CLERK_AUTHORIZED_ORIGINS` | JSON array of the editor's exact origins, such as `["https://app.example.com"]`. Exact `http://localhost`, `127.0.0.1` and `[::1]` origins are accepted for local development |
| `KONAE_CLERK_BETA_SUBJECTS` | JSON array of admitted Clerk user IDs, such as `["user_example"]`. An empty array admits nobody |
| `KONAE_CLERK_PUBLISHABLE_KEY` | Optional. The `pk_test_` or `pk_live_` key the sign-in page loads Clerk with |

The operator decides who gets in. A new person signs in to Clerk with Google, and Konae refuses the exchange with "Sign-in failed or this account has not been admitted to the beta." The operator finds that person's user ID in the Clerk dashboard, adds it to `KONAE_CLERK_BETA_SUBJECTS`, restarts Konae, and the person signs in again.

## What Konae checks

The browser posts Clerk's session token to `/_konae/owner/clerk`. Konae accepts it only with an RS256 signature from a key in the file with the same key ID, the configured issuer, an authorized party equal to the request's origin, a lifetime of at most five minutes, valid expiry and not-before times, a `user_` subject and a `sess_` session ID, and a session that is not pending. It refuses unknown keys and critical header extensions.

The first exchange maps the issuer and subject to a new owner ID in `owners.sqlite`. The mapping never reads an email, so two Clerk instances with the same subject never share an owner. Konae then sets its own owner session cookie for one hour and stores only the hash of its token.

With Clerk on, every agent credential lasts at most one hour, and Konae checks admission on every owner request and when it redeems an OAuth authorization code. Removing a subject from `KONAE_CLERK_BETA_SUBJECTS` or suspending the owner stops both at once. Signing out deletes the current session only.

Deleting or suspending the account in Clerk revokes nothing in Konae, because Konae has no Clerk webhook. The session ends within an hour, and a credential issued near its end can last another hour. To stop access at once, remove the subject or suspend the owner.

## Suspend an owner

```sh
KONAE_DATA_DIR=/data KONAE_CLERK_ISSUER=https://example.clerk.accounts.dev \
  konae creator-suspend user_example
```

The command suspends the owner that issuer and subject map to and deletes their sessions in one transaction, then deletes their agent credentials and open OAuth codes in `mcp.sqlite`. Projects and app accounts stay. If the second step fails, the suspension still holds, because every owner request checks it. No command reverses it.

## Rotate keys

When Clerk rotates its signing keys, replace the JWKS file. Keep both keys in it during the rotation and remove the old one afterwards. Konae reads the file on every sign-in and owner request, so the new file applies at once. A token signed with a key the file lacks fails with "Unknown Clerk signing key."

## Test it

`cargo test --manifest-path kernel-rs/Cargo.toml -p konae clerk::tests` signs tokens with the test key in `kernel-rs/test-fixtures/clerk/` and checks signatures, claims, origins, the mapping, session expiry, admission, suspension and the cleanup of credentials. Never configure that key on a real instance. The tests use no Clerk account, so Google consent, redirect URLs, live claims and key rotation need a real Clerk instance to check.

The checks follow Clerk's [manual JWT verification](https://clerk.com/docs/guides/sessions/manual-jwt-verification) and [session token claims](https://clerk.com/docs/guides/sessions/session-tokens).
