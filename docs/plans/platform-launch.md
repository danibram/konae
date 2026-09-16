# Littlebox launch plan: opening an installation to other people

Revision of November 6, 2026. This revision replaces the one of the same date that made a third-party identity provider the creator sign-in. It sets the scope, behaviour and order of delivery for the first hosted beta. It does not claim these functions exist and does not by itself change an accepted ADR. Where it differs from the [signup and invitations plan](signup-and-app-invitations.md), this document decides.

It comes after the [self-use plan](self-use-and-sdk-modules.md). That plan leaves a single operator with a trusted installation, copies off the volume and an SDK with a module contract. This one lets other people create apps on that installation and lets strangers sign up to those apps.

## What we launch

A beta where creators sign up with an email, a password and an invitation code the operator issued. One owner per project. Apps are created from the editor or from an agent over MCP. Apps have their own accounts, open or invitation-only signup, private data and an export that runs on the same Kernel on another machine.

Littlebox depends on no identity provider. The Kernel issues creator accounts and app accounts with one verified-account implementation in two scopes. An OIDC provider may be attached as an optional adapter later; signing in and running an installation never require one. The earlier integration with a hosted provider stays in the tree as that optional adapter and is documented in [hosted creator access](../clerk-creator-access.md).

Littlebox hosts a static frontend, declared actions, SQLite and files. Each app is one folder on the Kernel's volume, with its compiled artifact, its databases and its files, as the [self-use plan](self-use-and-sdk-modules.md) lays out; a copy of the folder is the app. The first version runs no server supplied by a creator. The Validation Gate and the query bound hold on the platform and on the exported Kernel.

Generation keeps the current modes: the creator's own provider credentials in the browser, or an external agent. No AI credit plan and no automatic billing. Quotas for projects, storage, operations and email are configured and shown before people are admitted. Reaching one produces a visible error; it never deletes data or blocks a download.

Proposed first values: 50 invited creators, three hosted apps per creator, 100 access emails per creator per day and 1,000 per installation. Each recipient takes at most ten a day and three a minute. Storage quotas stay as they are and their use is shown. The operator raises a limit only after checking use, capacity and the contracted limits of the email provider; a queue cannot exceed the provider's maximum.

## Decisions closed for this version

| Area | Decision |
| --- | --- |
| Creator identity | Email and password in the Kernel, verified, admitted with an operator-issued invitation code. Internal ID and permissions in Littlebox |
| Operator of an installation | Local owner account, created with a new installation secret; the secret is not a creator signup path once invitation codes exist |
| App users | Email and password in the Kernel, isolated per Deployment |
| Verification | Required before a new account of either scope is active |
| Signup | Open, Invitation only or Closed; the existing Allowlist stays as a compatible advanced option |
| Invitations | By email, single use, seven days |
| Recovery | Emailed link, 30 minutes; invalidates earlier sessions |
| Hosted email | Resend, configured and paid by the operator of Littlebox |
| Email on another machine | The new operator's credentials, through Resend or SMTP |
| Authorization | Action and row rules in the Kernel; signing in grants no rows |
| Creator teams | One owner per project; collaboration and ownership transfer deferred |
| App user sign-in methods | Social sign-in, passkeys and SSO deferred |
| Migration | Full export with an explicit write pause; no continuous replication |
| First support outside Littlebox | Docker kit with pinned versions and a persistent volume, a Kernel the operator can administer |
| Isolation of hosted code | App responses carry a Content Security Policy, frame restrictions and HSTS; administrative cookies never reach an app origin |

No external account enters an export or is needed to sign in to a migrated app. The owner can administer their projects' data; row privacy separates app users from each other, not from the operator of the server.

## One identity implementation, two scopes

`kernel-rs/src/auth.rs` already issues verified-less accounts for app users and the owner page issues owner accounts behind the installation key. This plan makes one account core serve both.

| Concern | Creator scope | App user scope |
| --- | --- | --- |
| Store | `owners` in the installation's identity database | `identity.sqlite` per Deployment |
| Admission | Invitation code from the operator | The Deployment's signup policy |
| States | pending, active, suspended | pending, active, suspended |
| Challenges | verification, recovery | verification, invitation, recovery |
| Session | Administrative cookie on the editor host, MCP credentials | App cookie on the app host |
| Suspension | Operator command on the server; kills sessions, codes and MCP credentials | Owner tool; kills sessions, keeps rows |

A challenge carries its purpose and scope. A recovery token does not accept an invitation, and a token from one Deployment does nothing on another or on the creator scope. Opening a link with GET changes nothing; the person confirms with POST.

The operator issues invitation codes from the server with the Kernel executable and can list, revoke and see which creator each code produced. A code is single use and expires. The installation key keeps creating the first owner on a fresh installation and is refused for signup once a code system is active, so a leaked key does not open creator signup.

## End-to-end product flow

1. The creator receives an invitation code, signs up with email and password, verifies the email and enters. Littlebox creates the internal identity once.
2. They describe the app in the editor or authorize their MCP agent. The connection returns to where it started after sign-in and shows the permissions granted.
3. The agent creates the app with the SDK. It uses simulated identities only in the preview and tests the rules with two different users.
4. Before publishing, the creator chooses visibility and who may sign up. A request that already states both lets the agent run those steps.
5. The Kernel publishes with the final policy from the first request. A private app with Invitation only needs at least one invitation before anyone can enter; the owner may invite their own email without becoming an app user automatically.
6. Under Open, the person enters email and password and verifies the email. Under Invitation only, they open the invitation and set their password. An existing account must sign in; the invitation does not change its password.
7. The creator manages users, suspensions, invitations and policies from the editor or MCP. Results distinguish email pending, accepted by the provider, delivered to the recipient's server and failed.
8. The creator can download the code, Eject or migrate the app. Each operation explains what happens to the hosted app before it runs.

## Download, Eject and migrate

| Visible operation | Code | Hosted app and users | Result |
| --- | --- | --- | --- |
| Download code | Copy of the files and development configuration | Unchanged | Files without real data, accounts, tokens or secrets |
| Eject to edit outside | The project goes from Managed to External | Keeps URL, users, sessions, data, invitations and email service | A conventional project that publishes artifacts through MCP |
| Download a full copy | Consistent copy of one revision and its state | Keeps running once the capture ends | Private package to restore or to prepare a migration |
| Migrate to another server | Code and full state with the Kernel | Paused, then retired at the origin | A new installation receives the writes |

Eject does not cancel hosting, does not necessarily export data and does not close the creator's account. Email keeps working while the app is hosted. Projects created externally over MCP are already External and do not pass through Eject.

Eject runs on the server with an expected revision. Before the project changes, the client fetches and checks the code package. The operation is idempotent and lets the package be downloaded again after an interruption. From then on the server refuses Managed source changes, including those from old tabs or agents. The external agent can keep publishing valid artifacts. Returning to the managed editor creates a new project, with no automatic merge of production data.

## What a full copy carries

| Kept | Replaced or excluded |
| --- | --- |
| Code, manifest, actions, schema and migrations | Sessions and MCP credentials of the origin |
| Published artifact and a compatible Kernel version | Administrative accounts and invitation codes of the origin |
| Business data and files with their owners | Email, storage and AI keys of the operator |
| User IDs and password hashes | Invitation, verification and recovery tokens |
| Verified state, suspension and access rules | Email jobs not yet run |
| Visibility and signup policy | Cookies, signing keys and trusted origins of the origin |
| Invitation history and pending recipients | Secret integrations the owner must configure again |

The package names the published revision and the code revision. Unpublished changes are marked as such; a draft is never presented as the code that produced the app in production. External projects supply their sources from the owner's machine or repository; Littlebox does not claim to keep sources it never received. Without them it can deliver state and a runnable artifact, not a complete project export.

The export uses a versioned manifest, checksums and an explicit content list. Accounts keep their IDs and hashes; permissions that use internal Deployment IDs are remapped to the destination without changing user IDs or row and file owners.

The package holds personal data and password hashes. Downloading it requires a recent administrative session or an explicit `export_data` delegation, distinct from reading metadata. It is delivered authenticated, with an expiry, and is never added to a code repository. Remote copies are encrypted and downloads bypass logs and analytics.

The importer checks integrity, version, references and limits before activating anything. It installs into an empty destination with a resumable, idempotent operation and publishes the state only when finished. It does not merge users by email or overwrite an existing app. Restoring over an installation is a separate procedure with a prior copy.

## Production migration

1. Prepare the destination with the pinned kit, a persistent volume, HTTPS, a public origin, a local owner and an email transport. Keep it in preparation, reachable only by the operator, with no automatic sending.
2. Rehearse an import and check compatibility, space, files and startup. The rehearsal accepts no user traffic and sends no invitations.
3. Start the migration from the origin with an operation ID. Pause new writes, signups, recovery, administrative changes and sending for that app. Finish in-flight operations and wait for or resolve pending sends.
4. Capture the final state under one lock covering data, identity, files, policy and publication. Record revision and checksums. The origin stays in maintenance until the operation completes or is cancelled explicitly; a restart does not reactivate it.
5. Import the final state into an empty destination or replace the rehearsal through the controlled procedure. Check counts, integrity and access with test accounts the owner controls, without changing real users' passwords.
6. Confirm at the origin that the migration is ready. The origin stays without writes and keeps a receipt. The operator activates the destination, configures the domain or announces the new URL. The beta accepts a maintenance window.
7. Users sign in again with their password. The destination resends pending invitations explicitly with new tokens and links. It does not replay the origin's queue.
8. Keep the origin in maintenance for seven days for inspection and recovery. Delete its active state after that; remote copies expire under their 30-day retention. The interface shows both dates.

If preparation or import fails before writes are enabled at the destination, the owner can cancel and resume the origin. Invalidated access challenges must be issued again. If the destination has already received writes, going back requires pausing it and migrating its updated state; an old copy is never reactivated.

Littlebox controls its origin and offers this procedure in the exported Kernel. It cannot stop whoever owns the server from starting other copies. That is why an import starts disabled and the procedure states which installation receives writes. No synchronisation or merge between divergent copies is promised.

## Cases the exit must resolve

| Case | Decided behaviour |
| --- | --- |
| Eject and stay hosted | Accounts, sessions, email and URL continue; who maintains the sources changes |
| Download a copy without migrating | The origin continues; any import stays disabled until the operator enables it |
| Clone only to develop | Code and fake data; real accounts are not copied by default |
| Run two copies with real data | Independent installations; a password or suspension does not sync |
| Keep the domain when migrating | Users keep the address but sign in again; keys and sessions rotate anyway |
| Change domain | Accounts, passwords and data stay; a new URL and new bookmarks |
| Use a URL under the Littlebox domain | The subdomain is not exported; the migration page may show the validated new URL for seven days |
| Open an old access link | Refused; its token is not forwarded and is not consumed elsewhere |
| Pending invitation | The recipient is kept, not the token; the operator decides when to resend |
| Signup pending verification | Kept as pending, without access; needs a new challenge |
| Suspended account | Stays suspended at the destination and gains no access through an invitation |
| A person in several apps | Independent accounts; migrating one changes none of the others |
| Creator account closed | App user accounts are not deleted; administration is revoked and the creator recovery procedure opens |
| Delete the hosted app after migrating | Cannot delete the external copy; removes only origin resources under their retention |
| Destination without email | The operator may enable a limited mode explicitly; active users enter, signup, invitations and recovery are unavailable with a visible explanation and the saved policy unchanged |
| Email provider down | Jobs are kept and retried; no user is activated unverified and no delivery is claimed |
| Unverified domain or sender | The preflight check blocks sending; the rest of the configuration can be rehearsed |
| Corrupt package or incompatible Kernel | Refused before activation; origin unchanged, or still in maintenance if the migration had begun |
| Repeated package | The same import returns its receipt; no duplicate users or data |
| Export while writes arrive | Consistent capture through a short pause; for a migration the pause stays until the end |
| Role or policy change during export | Serialised with the capture; no mixed revisions |
| New publication after import | Keeps the destination's accounts and data; does not reapply the initial state |
| Back from another machine to Littlebox | Assisted production import deferred; the standard package is kept for a future migration |
| Replace the Kernel with another backend | The owner has sources and data and must migrate authentication and rules. Guaranteed continuity uses the exported Kernel |

The first hosted version uses Littlebox subdomains. Hosted custom domains are deferred. The Kernel outside Littlebox does let its HTTPS origin be configured. No POST or authentication route is redirected during a migration; the origin page offers a clean link to the new app.

## Security

### Isolation of hosted code

Once other people publish, hostile JavaScript runs on `<slug>-<id>.<suffix>`. The Kernel sends no security headers today. Before admission:

- Every app response carries a Content Security Policy that allows the app's own origin and the Kernel runtime endpoint, blocks inline event handlers and foreign scripts, and sets `frame-ancestors` so an app cannot be framed by another origin and the editor cannot be framed by an app.
- HSTS on every host behind TLS. The administrative cookie keeps the `__Host-` prefix so a `Domain=<suffix>` cookie set by an app never reaches the editor. App cookies use the same prefix on their own host.
- The editor and the Admin Interface stay on an origin no app shares. A request from an app origin to administrative MCP is refused on origin, not only on credential.
- Tests cover cross-app requests, administrative MCP calls from an app, forged `Origin` and `Host`, cookie injection from an app subdomain, and a framed editor.

### Access and email

New accounts go from pending to active after proving control of the email. The verification link must not activate a password chosen earlier by someone else who registered that address: bind verification to the attempt and the browser, or require setting the password again before activation. A GET from a mail scanner never changes state.

An invitation fixes the email and the app, is consumed once and rechecks policy and suspension inside the acceptance transaction. A challenge from one app is useless on another. Resending invalidates the earlier token. Closed blocks every pending signup; suspension invalidates sessions and every action checks the user is still active.

Legacy accounts without verification are not trusted for having passed an email list. Before the beta opens, migrate them to verification required, revoke their sessions and require verification and a new password to enter. Do not change IDs or reassign rows. Test installations without email stay in local mode and are not promoted to production with active unverified accounts.

Invitations and their email job are stored atomically. The queue persists, encrypts transient secrets and uses idempotency keys; it records and verifies provider events. Limits apply per creator, app, IP and recipient, and a global send limit. Account creation never makes Littlebox a general email relay.

The Kernel issues the creator's administrative session and checks its permissions. Accounts are never joined by matching email. Sign-out, suspension and revocation have defined effects on own sessions and MCP delegations; an identity that was closed is not accepted indefinitely. Recovery of an installation's owner uses a local procedure with server access, not a password shared between exports.

## Operating requirements before admission

### Sources and project ownership

The editor already saves Managed sources on the server per owner and revision, and the browser keeps a cache. What remains: a second authenticated browser recovers the same project, concurrent changes produce a visible conflict and never overwrite work silently, importing local drafts requires the owner to pick them, and signing out leaves no private cache for the next person on the browser. External projects appear in the list; their sources stay on the owner's machine.

### Copies and recovery

The self-use plan delivers the scheduled copy off the volume. Before admission it is extended to identity, files, policies, owner metadata and Managed sources, encrypted, with an hourly copy, 24 hourly copies kept and one daily for 30 days. A restore rehearsal on a new machine reaches a recovery point of one hour and a recovery time of four hours. These are rehearsal criteria, not an announced SLA.

Alerts cover a failed copy, a full disk, email failure and a failed restore. After a restore, sessions and challenges are invalidated. The record of suspensions, revocations and deletions is persisted off the volume before those changes are confirmed and reconciled before reopening; if it cannot be recovered, the installation stays in maintenance. A restore never resurrects a deleted or suspended account.

Closing a creator's account is an explicit operation that shows the apps affected. The beta has no unpaid-bill state because it bills nothing. An operational suspension shows its reason and keeps an administrative path to export where appropriate.

### Portability within the quotas

The exporter combines state into a package of up to 16 MiB, while quotas let a database grow to 256 MiB, identity to 64 MiB and files to 20 MiB. An app the host accepts can stop being exportable. Before launch, replace the base64 JSON package with chunked transfers and a manifest, with per-component limits and a space check. Export and import cover the maximum quotas, also when the write quota is exhausted. The small existing format stays readable. The gap is not closed by hiding data or exporting only the accounts that fit.

## Deliveries

Each delivery has a verifiable result and the refusals it must produce. Dates are not estimated until the first delivery runs.

| Delivery | Work | Done when | Must refuse |
| --- | --- | --- | --- |
| A. Verified account core | States, challenges with purpose and scope, verification, recovery, suspension, legacy migration in `auth.rs` and `main.rs`; the same core behind the owner page | A person of either scope signs up, verifies and enters; a suspended one cannot | A pending or suspended account getting a session; a challenge used twice, on another scope, after expiry or by GET; a third party's password surviving verification |
| B. Email transport and queue | Persistent queue, Resend adapter, SMTP, development mailbox, provider events, limits | A queued message reaches a real inbox and its state is visible; a provider outage leaves it retryable | Sending with an unverified sender; exceeding a recipient, creator, app or global limit; a replayed or unsigned provider event |
| C. Creator admission | Invitation codes issued, listed and revoked on the server; signup with code; installation key no longer a signup path; operator suspension for local accounts | A new creator enters with a code and without the installation key | A used, expired or revoked code; the installation key at signup once codes are active; a suspended creator's sessions, codes and MCP credentials |
| D. Hosted code isolation | CSP, `frame-ancestors`, HSTS, cookie prefixes on app hosts, origin checks on administrative MCP | The tests in the isolation section pass against the Docker stack | An app framing the editor; an app cookie reaching the editor host; administrative MCP from an app origin |
| E. App invitations and policies | Invitation only, invitation accept, resend, revoke, policy recheck in the acceptance transaction | An invited person sets a password and enters; an uninvited one cannot create an account | Acceptance under Closed; acceptance for a suspended account; two concurrent acceptances producing two accounts |
| F. MCP user tools | `manage_users` and `export_data` permissions, `app_users_*` and `app_invitations_*` tools, idempotency keys, audit, initial policy in `release_plan` | An agent creates a private app, invites two people and both enter with no hidden administrative step | A project-limited token touching another project's users; `publish` changing policy without `manage_users`; a retried request creating a second invitation |
| G. Editor screens | Users view, invitations, email states, publish dialog with visibility and signup, access pages for signup, pending verification, expired invitation and recovery | A creator does from the editor everything delivery F does over MCP | Messages that confuse creating an app with signing in to one |
| H. Chunked export and import | Manifest, checksums, per-component limits, resumable import into an empty destination, `export_data` delegation | An app at its maximum quotas exports and imports without omitting data or loading the package into memory | Import over an existing app; a package with a bad checksum; export without `export_data` |
| I. Eject on the server | Managed/External state on the server, expected revision, idempotent code package, refusal of Managed writes afterwards | Eject keeps production serving and refuses a Managed write from an old tab | A Managed write after Eject; Eject with a stale revision |
| J. Migration and recovery | Persistent maintenance, receipts, activation, domain, own email, cancellation, encrypted remote copies | A rehearsed migration between two installations; a full restore on an empty volume within the rehearsal targets | Two writers active through the supported procedure; a restore that revives a revoked identity |
| K. Beta operation | Quotas shown, alerts, account closing, documentation, production email, end-to-end rehearsal | Every opening criterion met with recorded evidence | Admitting a creator before a criterion is met |

### Dependencies and parallel tracks

| Track | Deliveries | Waits on |
| --- | --- | --- |
| Identity | A, then C and E | nothing |
| Email | B | nothing; A and B join at verification |
| Isolation | D | nothing |
| Export | H, then I | nothing; J needs H and I |
| Tools and screens | F, then G | A, B, E |
| Operation | J, K | everything above |

Identity, Email, Isolation and Export touch disjoint files and can run at the same time. F waits for the app user states of A and E and the queue of B. G waits for F. J and K close.

Every delivery that changes the Kernel's contract updates `docs/mcp.md`, `CONTEXT.md` and the affected ADR in the same change. Creator identity in the Kernel and HTTP bindings on anonymous actions both amend ADR-0002; a new ADR records both rather than a third dated note.

## Criteria for opening the beta

- A new creator signs up with a code, connects MCP and publishes an app from scratch. The editor allows the same with the same checks.
- Open signup and Invitation only work with real email; recovery, expiry, resend and suspension have failure tests.
- Two users cannot read or change each other's private rows. Administrative and runtime access stay separate in tools, cookies and credentials.
- Every isolation test in the security section passes against the Docker stack.
- A Managed project reappears with its last saved sources in another browser; a change of account reveals no other person's drafts.
- Eject keeps the hosted app and refuses new Managed writes, including from old clients.
- An app at its maximum quotas exports and imports without omitting data or exhausting memory.
- A migration keeps IDs, passwords, data, files, suspension and policies. Users sign in again; origin tokens fail.
- The destination works with the Littlebox host unreachable. Its owner administers the app, configures email and recovers accounts with no external provider.
- Failing mid-migration allows a retry or a cancel without two active writers. Returning after writing at the destination requires a new migration.
- A volume loss recovers from remote copies within the rehearsed targets without reactivating revoked identities.
- Build, Kernel, MCP integration, security and browser checks pass. The exit path runs on two distinct installations, not on mocks.

## Deferred

Creator teams, ownership transfer, automatic billing, arbitrary visual roles, hosted custom domains, SSO, passkeys, social sign-in for app users, generic business notifications, replication between installations, migrations without maintenance, data merging, assisted production import back into Littlebox, and any identity provider as a requirement.

These must not appear as active controls or beta promises. First-version support outside Littlebox is the documented kit with the Kernel; replacing it with another backend is the owner's work.

## Basis

- [Self-use and SDK modules plan](self-use-and-sdk-modules.md), the installation this plan opens.
- [Signup and invitations plan](signup-and-app-invitations.md), access detail, subordinate to this document.
- [Hosted creator access](../clerk-creator-access.md), the optional identity adapter.
- [ADR on Eject and sources](../adr/0005-build-artifacts-separate-managed-and-external-projects.md).
- [Architecture](../architecture.md), Kernel storage and the absence of state replication.
- [MCP](../mcp.md), implemented catalogue and limits.
- `src/lib/managed-sources.ts`, server-side Managed sources.
- `src/lib/export-project-kit.ts` and `kernel-rs/src/main.rs`, portable state and current limits.

As each delivery lands, the affected ADRs, MCP contracts and documentation are updated, and the evidence is recorded here without describing pending decisions as the current system.

## Evidence

### Delivery D, November 6, 2026

The Kernel sends the isolation headers from `kernel-rs/src/security.rs`; [the architecture](../architecture.md#isolation-of-hosted-code) lists the policy. The cookie prefixes and the refusal of administrative MCP on app hosts already existed. This delivery adds the headers, an explicit refusal of an app `Origin` on administrative MCP, owner sign-in and OAuth, and the tests.

| Isolation test | Where | Run against |
| --- | --- | --- |
| Cross-app runtime request and session reuse | `tests/security-http.integration.test.ts` | Local Kernel, HTTP |
| Administrative MCP from an app origin, with and without a credential | `tests/security-http.integration.test.ts` | Local Kernel, HTTP |
| Forged `Origin` and `Host` | `tests/security-http.integration.test.ts` | Local Kernel, HTTP |
| Cookie planted with `Domain=<suffix>` | `tests/security-auth.integration.test.ts` | Local Kernel, HTTPS origin |
| Framed editor and framed app | `tests/security-http.integration.test.ts` | Local Kernel and the Vite config |
| CSP on app pages, sub-resources, the fallback route and the sign-in page | `tests/security-http.integration.test.ts`, `kernel-rs/src/security.rs` | Local Kernel |
| HSTS on HTTPS, none on HTTP | `tests/security-auth.integration.test.ts` | Local Kernel |

A headless Chrome session loaded all eight product-fit examples and the docs guide page from a local Kernel. Each page ran its inline runtime, and Chrome reported no policy violation. Injecting an `onerror` attribute, an inline script and a fetch to another origin into the live poll app produced three violations, and none of them ran. The editor's `srcdoc` preview rendered under `frame-ancestors 'none'`. A starter published from the editor loaded its Google Fonts and ran. That session used a scratch script; the replay's own browser pass in `smoke.ts` has not run, because the Bash sandbox stops Chrome from starting.

These tests did not run against the Docker stack. That stack serves plain HTTP on `localhost:8080`, so the `__Host-` cookies and HSTS cannot be exercised there; the "done when" criterion needs an HTTPS gateway in front of the stack.

The plan did not account for two things. The preview applies no Content Security Policy, so an app that uses `onclick="..."` or a foreign script works in the preview and stops once published; the agent prompt now states the policy, and the preview needs the same policy on its own origin to close the gap. The starter templates import Google Fonts, so the app policy allows `fonts.googleapis.com` and `fonts.gstatic.com`; a stricter policy would render published starters in system fonts while the preview shows the web font.

### Delivery H, November 6, 2026

`package_export` in `kernel-rs/src/package.rs` captures one Deployment under its locks and offers its database, accounts and files as three download transfers with a versioned manifest. A release imports them as `initialPackage` into a project with no Deployment. [ADR-0012](../adr/0012-an-app-leaves-as-a-chunked-package.md) records the decision and [the MCP guide](../mcp.md#export-and-import-an-app) the format. The CLI's `export --with-data` and the kit's `deploy.mjs` use it.

| Refusal or proof | Where | Run against |
| --- | --- | --- |
| Import over an existing app | `tests/package.integration.test.ts` | Local Kernel, MCP |
| Upload bytes that differ from their SHA-256; an upload that differs from the manifest; manifest version 2 | `tests/package.integration.test.ts`, `kernel-rs/src/package.rs`, `src/lib/portable-package.test.ts` | Local Kernel, unit |
| Export without `export_data` from `read`, `publish`, and `read`+`write`+`publish` credentials, and from an `export_data` credential for another project | `tests/package.integration.test.ts` | Local Kernel, MCP |
| A `read` credential reading or inspecting an export's transfers | `tests/package.integration.test.ts` | Local Kernel, MCP |
| Component over its quota | `tests/package.integration.test.ts`, `kernel-rs/src/package.rs` | Local Kernel, unit |
| Not enough disk | `kernel-rs/src/package.rs` (message and arithmetic only) | Unit |
| Sessions, email links and API keys absent from the export and deleted on import | `kernel-rs/src/package.rs`, `tests/package.integration.test.ts` | Unit, local Kernel |
| A repeated apply returns its receipt and one Deployment exists | `tests/package.integration.test.ts` | Local Kernel, MCP |
| Kit export and import with a freshly compiled kit Kernel, `export_data` refusal in the CLI | `tests/smoke/portable-export.ts` | Two local Kernels |
| 256 MiB database at its write cap and 20 MiB of files | `tests/smoke/package-quota.ts` | Two local Kernels, debug build |

The quota run filled the database through owner SQL with random bytes until the Kernel refused a write with "database or disk is full", then exported with `exportLocal` and imported with the kit's `node scripts/deploy.mjs`. The database file was 256.0 MiB and the kit archive 276.7 MiB. Row counts and byte totals matched on the target, the app user signed in with the same password, all ten 2 MiB files read back, the old session failed, and the target still refused a write for lack of space. A `ps` sample every 100 ms put the peak resident memory at 33 MiB for the source Kernel, 94 MiB for the Bun process that ran the export and the test itself, 23 MiB for the target Kernel and 117 MiB for the Node deploy script. Export took 97 s and import 128 s on the debug build, almost all of it in 64 KiB MCP calls. The target's audit log held 4 `transfer_write` rows for the import.

Not verified: an identity file near its 64 MiB cap, because signups are rate limited; it goes through the same code as the database. The space refusal is tested as a function, not against a full volume. The run used the local Kernels, not the Docker stack.

The plan did not account for three things. Every credential of one owner shares the transfer principal, so making `project_export` need `export_data` was not enough: the export's transfers check `export_data` too, or a `read` token of the same owner could read them. Auditing each 64 KiB block would have written about 4,400 rows per import into a log capped at 10,000, so package transfers audit once per component. The editor's own export runs in the browser and still uses the 16 MiB format; an app past that size leaves through the CLI until the editor streams the package to disk.
