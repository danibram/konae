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

### Delivery C, November 6, 2026

`kernel-rs/src/admission.rs` issues, lists and revokes invitation codes from the Kernel executable and consumes a code in the transaction that creates the creator. The installation key creates only the first owner; [ADR-0011](../adr/0011-one-verified-account-core-for-creators-and-app-users.md) records the rule and why it is stricter than this plan's. The README's creator admission section is the operator guide, and [the MCP guide](../mcp.md#creator-admission) lists the routes and refusals.

| Refusal or result | Where | Run against |
| --- | --- | --- |
| A new creator enters with a code and without the key, and returns to the MCP client's OAuth consent | `tests/creator-admission.integration.test.ts` | Kernel executable, required verification, development mailbox |
| Used, expired, revoked, unknown and malformed codes; one code raced by two signups | `kernel-rs/src/admission.rs` tests; used, revoked and unknown also in the integration suite | Unit, and the Kernel executable |
| The key at signup once a code exists, once a second creator exists, and after every code is revoked | `kernel-rs/src/admission.rs` tests, `tests/creator-admission.integration.test.ts` | Unit, and the Kernel executable |
| A suspended creator's session, OAuth token, delegated credential, unexchanged OAuth code, recovery link, and a pending creator's verification link | `tests/creator-admission.integration.test.ts` | Kernel executable with `creator-account-suspend` |
| A code spent on an existing email answers like a new one and cannot be reused | `kernel-rs/src/admission.rs` tests | Unit |

A headless Chrome session against a Kernel from this branch showed the owner page asking for the setup key on an empty installation, asking for an invitation code once a code existed, and showing the pending-verification panel after a signup with a code. Platform access in the editor was type-checked and built but not opened in a browser.

The plan did not account for three things. "Once a code system is active" left signup open to a leaked key on an installation that never issued a code, so the key also stops after the first creator. The browser tests create an owner per run with the default development key, so that key keeps working until the first code is issued. "Codes" in the suspension row reads here as the creator's open account links and OAuth authorization codes, because operator-issued invitation codes are never held by a creator.
### Delivery B, November 6, 2026

The queue, the worker, the transports, provider events, the preflight and the limits are in `kernel-rs/src/mail.rs`; [ADR-0012](../adr/0012-account-email-goes-through-a-persistent-queue.md) records the decisions and [the README](../../README.md#account-email) the configuration.

| Refusal or guarantee | Test in `kernel-rs/src/mail.rs` unless named |
| --- | --- |
| Sending with an unverified sender: nothing queued, 503, sending stops when Resend answers 403 | `an_unverified_sender_stops_sending_and_enqueues_nothing`, `resend_preflight_needs_a_verified_domain_and_a_key_that_can_read_it`, `resend_outages_retry_and_an_unverified_sender_refuses` |
| Exceeding a recipient limit, the same for an address with no account | `the_recipient_limits_refuse_visibly_and_the_same_for_unknown_addresses`, `the_signup_route_answers_429_past_a_limit` |
| Exceeding the daily recipient, creator, app, installation and client limits | `the_daily_recipient_creator_app_installation_and_client_limits_refuse` |
| An unsigned, forged or replayed provider event; events out of order; no event activates an account | `svix_signatures_are_checked_over_id_timestamp_and_raw_body`, `provider_events_are_signed_idempotent_ordered_and_never_activate_an_account`, `the_event_route_is_closed_without_a_queue` |
| A provider outage leaves the job retryable with backoff, bounded | `a_provider_outage_keeps_the_job_and_retries_with_backoff` |
| A pending job survives a restart; a restarted worker does not send twice | `a_pending_job_survives_a_restart_and_a_restarted_worker_does_not_send_twice`, `resend_sends_with_the_job_id_as_idempotency_key_and_dedupes_a_restart` |
| The challenge and its job commit together | `a_refused_outbox_rolls_back_the_challenge` |
| A replaced, ended or expired link is not mailed | `a_resend_supersedes_the_earlier_job_and_an_expired_link_is_not_sent`, `a_job_whose_challenge_is_gone_is_not_sent` |
| Links sealed at rest and dropped once accepted | `a_signup_queues_one_sealed_job_and_the_worker_sends_it`, `a_sealed_link_opens_only_with_its_job_and_its_secret` |
| SMTP sends with a stable Message-ID | `smtp_sends_with_a_stable_message_id_and_checks_the_connection` |
| A copy carries the queue; a restore sends none of its pending jobs | `kernel-rs/src/copies.rs`, `a_copy_carries_the_mail_queue_and_a_restore_sends_none_of_its_pending_jobs` |

Resend and SMTP ran against local fakes: an axum server that keeps one email per `Idempotency-Key`, and a plain SMTP responder on loopback. The "done when" criterion, a queued message in a real inbox, has not run: it needs a Resend account with a verified domain, or an SMTP server, and neither was available. SMTP over STARTTLS and implicit TLS has not run against a server either; only the plain loopback path did.

The plan did not account for three things. A Resend key created with `sending_access` cannot list domains, so the sender check needs a second key with domain access or it refuses. Counting the recipient limit on sent emails would tell an attacker which addresses have accounts, because an active address gets no email; the limit counts requests instead. The plan names no per-app or per-client limit; the Kernel uses 100 a day per app and 20 an hour per client address.

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

### Delivery E, November 6, 2026

Signup has a fourth mode, Invitation only, on both sides: `SignupMode::Invitation` in `kernel-rs/src/main.rs` and `signupModes` in `src/deployments/client.ts`, which the editor's Auth panel offers as Open, Invitation only, Closed and Allowlist (advanced). `kernel-rs/src/invitations.rs` issues, lists, resends, revokes and accepts invitations, and answers the seven admin tools behind the new `manage_users` permission. The tools that the plan lists under delivery F (`manage_users`, `app_users_*`, `app_invitations_*`, idempotency keys and audit) landed here because the refusals in this delivery's brief needed them.

| Refusal | Test |
| --- | --- |
| Acceptance under Closed, and under an Allowlist without the address | `invitations::tests::acceptance_rechecks_the_policy_and_a_refusal_keeps_the_link` |
| Acceptance for a suspended account, suspended before, during or after the invitation | `invitations::tests::a_suspended_account_is_not_reactivated_by_an_invitation` |
| Two concurrent acceptances producing two accounts, through the route and against the store without the Kernel's lock | `invitations::tests::two_concurrent_acceptances_produce_one_account` |
| A token from another Deployment, an expired token, a token replaced by a resend | `invitations::tests::a_token_from_another_deployment_an_expired_one_or_a_replaced_one_does_nothing` |
| A retried create producing a second invitation or email; a reused key with other arguments | `invitations::tests::a_retried_create_with_the_same_key_produces_one_invitation_and_one_email`, and over HTTP in `tests/mcp.integration.test.ts` |
| An invitation changing an active account's password | `invitations::tests::an_active_account_is_told_to_sign_in_and_keeps_its_password` |
| A link working after revoke, or a copy carrying a working link | `invitations::tests::revoking_ends_the_link_and_removes_the_unused_account`, `invitations::tests::a_copy_carries_recipients_and_history_but_no_working_link` |
| A project-limited token touching another project's users; `publish` changing the policy directly or through a republish; an older token gaining `manage_users` | `tests/mcp.integration.test.ts`, "an invited person enters an Invitation only app, and only manage_users decides who may join", and `mcp::tests::runtime_principals_never_see_owner_tools_and_anonymous_visitors_cannot_upload` |

The integration test runs the done-when criterion against a local Kernel with the development mailbox: a publish-only token releases a private app with Invitation only, a stranger's signup gets 403, an invited person reads the link from `mailbox.jsonl`, sets a password and runs an action.

This delivery leaves four things undone. The invitation email names neither the app nor the inviter, because `AccountMessage` carries only a link and delivery B owns the message templates and the queue; `delivery` reports only whether the transport took the message. Invitations and their mail are not stored in one transaction, because the seam has no queue to write into; the rows commit first and a failed hand-off shows `failed`. The editor has no Users view yet (delivery G), so a person who does not write code can choose Invitation only but cannot invite anyone without an agent. There is no runtime MCP tool to accept an invitation; the HTTP route is the contract.
### Delivery I, November 6, 2026

`kernel-rs/src/eject.rs` adds `eject_plan` and `eject_apply` behind the `manage_lifecycle` permission. The plan checks the expected saved-sources revision before any transfer exists and returns the code package; apply flips the project to External under the project lock and records who and when. The CLI's `eject` command and the editor's Eject dialog both check the package before they apply. An ejected project opens read-only in the editor, and its card says External. [ADR-0005](../adr/0005-build-artifacts-separate-managed-and-external-projects.md) has the dated note and [the MCP guide](../mcp.md#eject) the tools and refusals.

| Refusal or proof | Where | Run against |
| --- | --- | --- |
| A Managed write after Eject, from the tab that saved last, from a tab one revision behind, and from an agent's write token | `tests/eject.integration.test.ts` | Local Kernel, MCP |
| A Managed write from an old browser tab after another tab ejected | `tests/e2e/eject.e2e.ts` | Chrome, `dev:local` |
| Eject with a stale revision, refused with no transfer created; apply after a save made the plan stale; the CLI with `--revision 0` writing nothing | `tests/eject.integration.test.ts` | Local Kernel, MCP, CLI |
| Eject without `manage_lifecycle` from `read`+`write`+`publish` and `read`+`manage_users`+`export_data` tokens, and from a `manage_lifecycle` token for another project | `tests/eject.integration.test.ts` | Local Kernel, MCP |
| An applied plan reapplied returns the same receipt; planning again returns the applied plan and the same package | `tests/eject.integration.test.ts` | Local Kernel, MCP |
| The Deployment keeps serving, keeps its hostname, and an app user stays signed in with their rows across Eject and a later release from outside | `tests/eject.integration.test.ts`, `tests/e2e/eject.e2e.ts` | Local Kernel, Chrome |
| Returning to Managed through `project_create` refused with "create a new project", for an ejected project and for one created External; Eject of a project with no saved sources | `tests/eject.integration.test.ts` | Local Kernel, MCP |
| A tampered package, a file that differs from the content list, a path outside the folder, another revision | `src/lib/eject-package.test.ts` | Unit |
| Audit rows for plan and apply | `tests/eject.integration.test.ts` | Local Kernel |

Not covered: the Docker stack, and an Eject racing a save on two connections at the same instant. The race is closed by the shared project lock, not by a test that forces the interleaving.

The plan said the client fetches and checks the code package before the project changes, and both clients do. It did not say what the package is. The Kernel cannot build the self-host kit, which is assembled in TypeScript, so the package is the saved source tree with `littlebox.json` and a README in a JSON envelope with per-file SHA-256s; the CLI writes it as a folder and the editor as a tar. It also did not say whether an ejected project's sources stay readable. They do, frozen, because the editor needs them to show the project read-only.

### Delivery H, November 6, 2026

`package_export` in `kernel-rs/src/package.rs` captures one Deployment under its locks and offers its database, accounts and files as three download transfers with a versioned manifest. A release imports them as `initialPackage` into a project with no Deployment. [ADR-0013](../adr/0013-an-app-leaves-as-a-chunked-package.md) records the decision and [the MCP guide](../mcp.md#export-and-import-an-app) the format. The CLI's `export --with-data` and the kit's `deploy.mjs` use it.

| Refusal or proof | Where | Run against |
| --- | --- | --- |
| Import over an existing app | `tests/package.integration.test.ts` | Local Kernel, MCP |
| Upload bytes that differ from their SHA-256; an upload that differs from the manifest; manifest version 2 | `tests/package.integration.test.ts`, `kernel-rs/src/package.rs`, `src/lib/portable-package.test.ts` | Local Kernel, unit |
| `package_export`, `project_export` and `database_export` without `export_data` from `read`, `publish`, and `read`+`write`+`publish` credentials, and from an `export_data` credential for another project | `tests/package.integration.test.ts` | Local Kernel, MCP |
| A `read` credential reading or inspecting an export's transfers | `tests/package.integration.test.ts` | Local Kernel, MCP |
| Component over its quota | `tests/package.integration.test.ts`, `kernel-rs/src/package.rs` | Local Kernel, unit |
| Not enough disk | `kernel-rs/src/package.rs` (message and arithmetic only) | Unit |
| Sessions, email links and API keys absent from the export and deleted on import | `kernel-rs/src/package.rs`, `tests/package.integration.test.ts` | Unit, local Kernel |
| A repeated apply returns its receipt and one Deployment exists | `tests/package.integration.test.ts` | Local Kernel, MCP |
| Kit export and import with a freshly compiled kit Kernel, `export_data` refusal in the CLI | `tests/smoke/portable-export.ts` | Two local Kernels |
| 256 MiB database at its write cap and 20 MiB of files | `tests/smoke/package-quota.ts` | Two local Kernels, debug build |

The quota run filled the database through owner SQL with random bytes until the Kernel refused a write with "database or disk is full", then exported with `exportLocal` and imported with the kit's `node scripts/deploy.mjs`. The database file was 256.0 MiB and the kit archive 276.7 MiB. Row counts and byte totals matched on the target, the app user signed in with the same password, all ten 2 MiB files read back, the old session failed, and the target still refused a write for lack of space. A `ps` sample every 100 ms put the peak resident memory at 33 MiB for the source Kernel, 93 MiB for the Bun process that ran the export and the test itself, 23 MiB for the target Kernel and 120 MiB for the Node deploy script. Each figure includes the process's own baseline, and the smoke fails above 192 MiB. Export took 81 s and import 99 s on the debug build, almost all of it in 64 KiB MCP calls. The target's audit log held 4 `transfer_write` rows for the import.

An identity file near its 64 MiB cap was not exercised, because signups are rate limited; it goes through the same code as the database. The space refusal is tested as a function, not against a full volume. The run used the local Kernels, not the Docker stack.

The plan did not account for three things. Every credential of one owner shares the transfer principal, so gating the export tools alone was not enough. The export's transfers check `export_data` too, or a `read` token of the same owner could read them. `database_export` handed the raw database to `read` and now needs `export_data` as well. Auditing each 64 KiB block would have written about 4,400 rows per import into a log capped at 10,000, so package transfers audit once per component. The editor's own export runs in the browser and still uses the 16 MiB format; an app past that size leaves through the CLI until the editor streams the package to disk.

### Delivery G, November 6, 2026

The editor's Auth section has three views: Rules, Users and Integrations. Users (`src/components/AppUsersView.tsx`) holds who can open the app, the signup policy with one sentence per mode and Allowlist under Advanced, invite by email in batches of up to 50, the invitation list with resend and revoke, and app accounts by state with pagination, suspend and resume. Integrations (`src/components/IntegrationsView.tsx`) sets and deletes signing secrets by name and creates, lists and revokes API keys with a curl for each route and the file read path. The publish dialog chooses visibility and signup policy on a first publish and shows both read-only on a republish. Every call is an admin MCP tool the Kernel already had; the Kernel did not change. The words and state rules live in `src/components/app-access.ts`.

| Refusal or proof | Where | Run against |
| --- | --- | --- |
| Messages that confuse creating an app with signing in to one, or an owner with an app user | `src/components/app-access.test.ts`, "speaks of app accounts and never of creating an app when it means joining one" | Unit |
| Invite and resend while the installation cannot send; the reason for no transport, a refused sender and a running check | `src/components/app-access.test.ts`, "mail readiness" | Unit |
| A retried batch producing a second invitation: the same addresses reuse the idempotency key | `src/components/app-access.test.ts`, "a retry of the same addresses reuses the idempotency key" | Unit |
| More than 50 addresses, a secret name or value `secret_set` would refuse, a key with no scope | `src/components/app-access.test.ts` | Unit |
| A republish sending `initialSignupPolicy` | `src/deployments/client.test.ts`, "first publish signup policy" | Unit |
| An uninvited visitor finding a way to create an account on a private Invitation only app; the done-when criterion from the editor | `tests/e2e/local-platform.e2e.ts`, "a private Invitation only app admits the person the owner invites from the Users view" | Local Kernel with the development mailbox, Chrome |

The browser test publishes a private app with Invitation only from the dialog, invites an address in Auth > Users, reads the link from `mailbox.jsonl`, accepts it, signs out, signs in with the chosen password, sees the invitation Accepted and the app account Active, then saves a signing secret and creates an API key whose curl names the app's host.

The plan asks the Users view to show each invitation as pending, accepted by the provider, delivered, failed or needing a resend. The Kernel records per invitation only whether the transport took the message (`queued`, `failed`, `not-sent`); the provider's accepted and delivered states exist per Email Job and as queue totals in `mail_status`, with no link from the invitation to its job. The view shows what the Kernel records and does not claim delivery. Joining an invitation to its Email Job is a Kernel change for a later delivery. Access pages served by the Kernel stayed out of scope.

### Delivery J, November 6, 2026

`kernel-rs/src/migration.rs` adds the maintenance state and the five migration tools; `kernel-rs/src/recovery.rs` adds the revocation record and its reconciliation; `kernel-rs/src/copies.rs` seals S3 copies with `LITTLEBOX_COPY_SECRET` and applies the record on restore. [ADR-0014](../adr/0014-an-app-migrates-with-one-writer-at-a-time.md) records the decisions, [the MCP guide](../mcp.md#migrate-an-app) the tools and refusals, and [the README](../../README.md#migrate-to-another-server) the procedure and the restore commands.

| Refusal or proof | Where | Run against |
| --- | --- | --- |
| Writes, signups, sign-in, recovery, invitation acceptance, HTTP routes and file reads on a paused app, each answered 503 with the operation; owner changes to policy, accounts, invitations, API keys, secrets, owner SQL and publishing refused; reads still open | `tests/migration.integration.test.ts` | Local Kernel, HTTP and MCP |
| The pause survives a restart, a scheduled copy and a restore; an unreadable state file pauses | `tests/migration.integration.test.ts`, `copies::tests::a_copy_carries_the_migration_state`, `migration::tests::the_marker_survives_a_restart_and_an_unreadable_one_pauses` | Local Kernel, unit |
| Email of a paused app is held uncounted and leaves after the pause; settling waits for a lock and for a job with the provider, within its bound, and ends the app's other queued jobs | `mail::tests::a_paused_apps_job_waits_and_leaves_when_the_pause_ends`, `migration::tests::settling_*` | Unit |
| `migration_begin` without `export_data`, without `manage_lifecycle`, or for another project's grant | `tests/migration.integration.test.ts` | Local Kernel, MCP |
| Two writers through the procedure: cancel after confirm, activate without the origin's confirmation, activate after the origin cancelled, begin on a confirmed origin or an inactive import | `tests/migration.integration.test.ts`, `tests/smoke/migration.ts` | Local Kernel; two Kernels |
| An import into an app that has one; the same package twice returns the first plan and receipt | `tests/migration.integration.test.ts`, `tests/smoke/migration.ts` | Local Kernel; two Kernels |
| A migration package in a version 1 reader; version 2 without a stamp, a stamp on version 1, a bad operation ID or hash | `package::tests`, `src/lib/portable-package.test.ts`, `tests/package.integration.test.ts` | Unit, local Kernel |
| `newUrl` over plain http, with credentials, query or fragment, or on the app's own host; the link shown only for seven days after confirmation; no redirect of POST or sign-in | `migration::tests`, `tests/migration.integration.test.ts` | Unit, local Kernel |
| `migration_status` never returns the confirmation | `migration::tests::the_status_view_never_shows_the_confirmation`, `tests/migration.integration.test.ts` | Unit, local Kernel |
| A restore that revives a suspended app account, a suspended creator or a revoked API key | `recovery::tests::a_restore_never_revives_a_suspended_account_or_a_revoked_key`, `copies::tests::a_restore_applies_the_revocation_record_or_holds_without_it`, `tests/smoke/migration.ts` | Unit; restore command on a fresh data directory |
| A restore whose record cannot be read starts in maintenance, refuses owner changes and reopens only through `reconcile` | `recovery::tests::an_unreadable_record_holds_the_installation_until_reconciled`, `tests/smoke/migration.ts` | Unit; Kernel executable |
| A suspension or revocation that cannot reach the copy target is reported, not confirmed | `recovery::tests::a_change_is_confirmed_only_once_it_is_off_the_volume` | Unit |
| An encrypted copy restored without its secret, with another secret, or with a file moved to another path; nothing installed | `copies::tests::an_encrypted_copy_restores_only_with_its_secret` | Unit, directory source |

The rehearsal, `bun run tests/smoke/migration.ts --port 19460`, ran on five Kernel processes from this branch. On the origin it published an app with two accounts, two rows, a file, an API key, a Signal, a pending invitation, a suspended account and Invitation only. `migration_begin` paused it; an import with one damaged component was refused while the origin stayed paused; `migration_begin` again returned the same operation and package; the retried import landed paused, a repeat returned the same Deployment, and a destination restart kept it paused, with both Kernels refusing a sign-in at that moment. After `migration_confirm` and `migration_activate` with `resendInvitations`, the destination had the same account IDs and states, the password signed in, both rows and the file read back, the suspended account stayed out, the policy was Invitation only, and the Signal was there. The origin's session, API key and invitation link failed on the destination; the resent link, on the destination's host, worked. The origin's page linked to the new address and refused sign-in without a redirect. A second migration cancelled before confirmation let its origin take a signup. A third Kernel with `LITTLEBOX_COPY_DIR` took a copy at restart, then suspended one account and revoked a key; `littlebox-kernel restore` into an empty directory took under a second, and the Kernel started on it kept the account suspended and the key revoked while the other account signed in. With `revocations/started.json` removed, the restored installation served only maintenance pages and refused an owner change until `reconcile --without-record`.

Commands and counts on this branch: `cargo test --manifest-path kernel-rs/Cargo.toml` 176 passed; `bun run test` 553 passed; `bun run build` passed; `bun run tests/smoke/portable-export.ts --port 19470` passed and compiled a fresh kit Kernel with the two new modules; `bun run tests/smoke/migration.ts --port 19490` passed; `bun run test:e2e:local` 7 passed, because the editor bundles `src/lib/portable-package.ts` and `src/lib/export-project-sources.ts`. Two earlier full runs under a load average of 10 to 15 from other work on the machine each lost one test elsewhere to a timeout; `tests/creator-admission.integration.test.ts` then passed alone.

Not run: encrypted copies against S3. The sealing in `send_to_s3` uses the same functions the directory test exercises, but MinIO was not running, so `tests/copy-restore.integration.test.ts` skipped its S3 case and no S3 copy was sealed or restored. The restore rehearsal measured a seconds-long restore of a small installation, not the one-hour recovery point and four-hour recovery time on a full volume. Nothing ran against the Docker stack.

The plan did not account for five things. "Domain" for a hosted origin can only be a link: the origin's page shows the validated new URL for seven days and the destination's own `LITTLEBOX_PUBLIC_ORIGIN` builds its hostname, so the app gets a new hostname on the destination even under the same domain. "Rotates signing material" has nothing to rotate: no signing key travels, sessions are random tokens, and route signing secrets live per project on the origin and never enter a package, so the destination sets new ones before the import publishes routes that need them. Activation therefore ends whatever sessions, links and keys the identity file holds and reports that. The procedure needed a way to stop a cancelled migration's package from ever serving, which the plan's order of steps did not give: the origin keeps a confirmation whose hash travels in the package and reveals it only at `migration_confirm`, after which cancel is refused. The plan says any import stays disabled until the operator enables it; this delivery pauses only migration packages, because the kit's `deploy.mjs` and the editor import version 1 packages that must serve at once, so a plain export imported elsewhere still makes an independent second copy, as "Run two copies with real data" describes. The plan says to delete the origin's active state after seven days; the Kernel shows the date and keeps the state, and deleting it is an operator step for now.
