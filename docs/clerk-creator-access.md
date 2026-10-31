# Hosted creator access

This integration is an optional adapter. Creator accounts are a Kernel concern with email, password, verification and operator-issued invitation codes, so signing in and running an installation never require this provider. The rest of this document describes the adapter as implemented.

Clerk proves the identity of a hosted creator. The Kernel maps the configured issuer and Clerk subject to an internal owner ID, then issues its own administrative session. The mapping never reads an email address. App users keep their own accounts, passwords and sessions in each deployment.

## Configure the operator account

Create a Clerk application and enable Google as its only sign-in and signup method. Disable other authentication methods in Clerk. The standard session JWT does not identify the social provider, so the Kernel cannot prove from that JWT that a person used Google.

Copy the instance's JWKS from its fixed Frontend API URL with `/.well-known/jwks.json` appended. Store the JSON in a file that only the operator can change. The Kernel reads that configured file. It never follows a URL supplied by a JWT or fetches a key based on an untrusted issuer.

Set these environment variables on the Kernel process.

| Variable | Value |
| --- | --- |
| `KONAE_CLERK_ISSUER` | Exact HTTPS instance origin, without a trailing slash |
| `KONAE_CLERK_JWKS_FILE` | Absolute path to the operator-managed JWKS JSON file |
| `KONAE_CLERK_AUTHORIZED_ORIGINS` | JSON array of exact editor origins, such as `["https://app.example.com"]` |
| `KONAE_CLERK_BETA_SUBJECTS` | JSON array of admitted Clerk user IDs, such as `["user_example"]` |
| `KONAE_CLERK_PUBLISHABLE_KEY` | Clerk publishable key for the hosted sign-in page |

Subject admission is an operator decision. A new person first creates their Clerk account with Google. The Kernel denies the exchange until the operator finds that account in the Clerk dashboard and adds its user subject to the beta list. The person then retries sign-in. This delivery does not send beta invitations or automate their acceptance. The pending screen should explain that access awaits operator approval and show the Clerk subject so the person can identify their request. An empty subject array denies everyone. An absent or invalid required setting produces a configuration error. Any Clerk variable enables hosted mode, so a partial configuration must never fall back to password authentication.

For local development, the authorized-origin list also accepts exact HTTP loopback origins. It rejects wildcard origins, paths, credentials and nonlocal HTTP. Self-hosted installations leave all Clerk variables unset and retain the local owner account protected by the installation setup secret.

## Verification and sessions

The exchange requires an RS256 signature from a configured RSA signing key and its exact key ID. The Kernel checks issuer, authorized party, request origin, expiration, not-before time and issuance time. Tokens must have a Clerk user subject and session ID, must not identify a pending session, and must expire within five minutes of issuance. Unknown keys, critical JWT extensions and malformed encodings fail closed.

Successful exchange creates the identity once under a unique provider and subject constraint. The provider includes the issuer, so identical subjects from different Clerk instances never become the same account. The Kernel stores only the SHA-256 hash of its random session token and gives the browser an administrative cookie. The administrative session expires after one hour. A Clerk JWT is never an app-user session.

Removing a subject from the beta list or suspending its identity blocks administrative sessions and delegated MCP credentials when the integration checks `owner_allowed`. `clerk::suspend` also deletes every Kernel browser session for that identity. It does not delete projects, deployments or app-user accounts. Logout deletes the current Kernel session; it does not revoke other browser sessions or delegated MCP credentials.

The operator can suspend a creator on the server using the same Kernel executable and data directory.

```sh
KONAE_DATA_DIR=/data KONAE_CLERK_ISSUER=https://example.clerk.accounts.dev \
  konae-kernel creator-suspend user_example
```

This command locates the owner by issuer and subject, suspends the identity and deletes its Kernel sessions in one transaction. It then deletes the owner's MCP credentials and pending OAuth codes. It does not start a server. If MCP cleanup fails, suspension remains in force because each administrative request checks the identity. There is no command to reverse suspension in this delivery. The operator must investigate the account before changing its stored status.

## Clerk deletion and key rotation

This delivery has no Clerk webhook or Backend API revocation lookup. Deleting or suspending an account in Clerk therefore does not immediately revoke a previously issued Kernel session. The one-hour administrative lifetime bounds that session. A delegated credential issued near its end can last another hour, so complete expiration can take up to two hours. Suspend the internal owner or remove beta admission when access must stop immediately. The hosted integration must cap all delegated credentials at one hour and must check admission when redeeming an OAuth authorization code.

The operator replaces the JWKS file when Clerk rotates keys. Include both active signing keys during rotation and remove retired keys when appropriate. Unknown key IDs fail closed with `Unknown Clerk signing key.` until the file contains the current public key. The configuration loader rereads that file for each authentication operation. The Kernel does not cache successful identities independently of session expiry.

## Local verification

`cargo test clerk::tests` verifies locally signed RSA tokens, tampered signatures, issuer and origin mismatches, expired and premature tokens, missing claims, algorithm substitution, unknown keys, pending sessions and beta denial. It also verifies stable internal identities, separate identities with the same email, session expiry, logout revocation, suspension and removal from the beta list. The checked-in private key under `kernel-rs/test-fixtures/clerk/` is an isolated test key. Never configure it as a deployed Clerk signing key.

A real Clerk instance is still required to check Google consent, allowed redirect URLs, live token claims, sign-in page loading, actual signing-key rotation and account closure behavior. Local tests use no Clerk account, production connection or secret.

The verification follows Clerk's [manual JWT verification](https://clerk.com/docs/guides/sessions/manual-jwt-verification) and [session-token claims](https://clerk.com/docs/guides/sessions/session-tokens). Clerk documents `azp` as the origin of the original Frontend API request. The Kernel requires that claim to equal the admitted origin of the exchange request.
