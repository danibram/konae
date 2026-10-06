# One verified account core for creators and app users

Status: accepted, 2026-11-06. Amends [ADR-0002](0002-kernel-owns-identity-and-visibility.md), and its HTTP update amends [ADR-0001](0001-separate-runtime-and-admin-data-access.md)'s rule that a private Deployment runs nothing without a login. [ADR-0012](0012-account-email-goes-through-a-persistent-queue.md) builds on it, and [ADR-0014](0014-an-app-migrates-with-one-writer-at-a-time.md) changes what a restore does with revocations made after a copy.

ADR-0002 put identity in the Kernel for app users. The launch plan admits creators the Kernel did not invite and strangers to their apps, so an address someone typed can no longer stand for a person. Both kinds of account need proof of email, recovery and suspension, and an export must carry them to a machine with no identity provider.

## Decision

The Kernel issues creator accounts and app accounts with one implementation, `kernel-rs/src/auth.rs`, in two scopes: `creator` in `owners.sqlite` and one scope per Deployment in its `identity.sqlite`. No external provider is needed to sign in or to run an installation. The hosted identity adapter stays optional and keeps its own accounts.

An account is pending, active or suspended, and only an active account gets a session. Session checks join on the state, so suspension ends live sessions without a sweep, and delegated MCP credentials and OAuth code exchanges read the creator's state on every use.

A challenge carries a purpose (verification, invitation, recovery) and a scope. The Kernel keeps its SHA-256 hash, an expiry and a consumed time. One transaction finds the challenge for that purpose and scope, marks it consumed, sets the password, activates the account and issues the session, so two concurrent answers produce one activation. Opening a link with GET changes nothing.

Verification requires choosing the password on the page the link opens. Signup in required mode takes only an email and stores no password. The plan allowed a second option, binding verification to the signup attempt and its browser. We did not take it because a link opened on a phone, or by an agent acting for the person, would then fail, and because it keeps two passwords in play: the one typed at signup and the one the verifier proves. With one password chosen after proof, a password a third party typed for someone else's address cannot survive, and verification, recovery and the coming invitation acceptance share one completion step: consume a token, set a password, enter.

`LITTLEBOX_ACCOUNT_VERIFICATION` chooses the mode for the installation. `local`, the default, keeps unverified signup so development without email works. `required` moves every account that never verified to pending, revokes its sessions, and keeps IDs and rows. The store records the mode it last ran under, so the move happens once per switch and again after a later local period.

## Considered options

- **Bind verification to the attempt and the browser.** Rejected for the reasons above.
- **Make verification required with no local mode.** Rejected because every development installation and test Kernel would need a mail transport before anyone could sign in.
- **Mark existing accounts verified during migration.** Rejected because nobody proved those addresses; an Allowlist only checked the text of an email.
- **Keep creator accounts in a separate store with their own code.** Rejected because the two scopes need the same states, links and refusals, and two copies would drift.

## Consequences

Required mode needs a mail transport; without one, signup and recovery answer 503 and existing active accounts still sign in. The transport is a seam (`AccountMail` in `kernel-rs/src/accounts.rs`); the only implementation today writes a development mailbox file. A full copy drops sessions and challenges, and an import deletes any it finds, so an old link never works on another machine. The operator suspends and resumes local creators with `littlebox-kernel creator-account-suspend` and `creator-account-resume`. Invitation acceptance and creator invitation codes plug into the same challenge model in later deliveries.

## Note, 2026-11-06: creator admission

Delivery C admits creators with invitation codes, in `kernel-rs/src/admission.rs`. The operator issues, lists and revokes them with `littlebox-kernel creator-code-create`, `creator-code-list` and `creator-code-revoke`. Codes live in `owners.sqlite` next to the accounts, as SHA-256 hashes with an expiry, a note, a revocation time, a use time and the creator they produced. `auth.rs` gained one seam: `request_signup_admitted` and `create_account_admitted` run an admission check inside the transaction that writes the account, so a code is consumed exactly when the pending creator is created, and a refusal rolls both back.

A code is not a challenge. A challenge proves control of an address and belongs to an account; a code proves the operator chose to admit someone and belongs to no account until it is used. The account a code creates still verifies its email through the challenge model above.

The installation key now creates an account only while `owners.sqlite` is empty and no code has ever been issued. The plan said the key stops "once a code system is active"; that rule alone left signup open to a leaked key on an installation whose operator never issued a code, so the key also stops once a first creator exists. The default development key is the exception: it keeps creating owners until the first code is issued, because local development and the browser tests create an owner per run, and the Kernel already refuses that key off loopback unless the operator opts in.

A code presented for an email that already has an account is consumed and creates nothing, and the answer matches a new email's, so a code cannot be used to test which creators exist. A verification link from a signup that began at the OAuth consent screen carries that consent path, so the creator returns to the MCP client after choosing a password.

## Update, 2026-11-06: HTTP bindings and API keys

The launch plan asks for one ADR that records both amendments to ADR-0002, creator identity and HTTP bindings on anonymous actions. This section is the second.

An action's access level decides whether its HTTP binding answers. Deployment visibility does not. ADR-0002 tied anonymous actions to public Deployments: a private Deployment runs nothing without an app session. That stays true for the shell, the runtime endpoint and every action called through the SDK. An action declared `anonymous` that also declares an `http` key answers on its path whatever the Deployment's visibility, and the signature it declares, when it declares one, is the check. An `authenticated` action may not declare `http`; the Validation Gate and the Kernel refuse that combination, so a route never needs a session cookie to reach rows that belong to a person.

A route has three kinds of caller, resolved before any SQL runs: a third-party service proves itself with the binding's signature, a program the owner wrote with an API key, and a person with the app session cookie. An API key is a third kind of Runtime Actor, next to the anonymous visitor and the signed-in app user. The owner issues it for one Deployment and names the actions and file visibilities it may use. The Kernel stores the SHA-256 hash only. An `actor` binding yields the key's ID, so rows a key writes have an owner. A key belongs to the Deployment, never to the owner, and grants no administrative capability.

Files gain one HTTP read path, `GET /_littlebox/files/<id>`, that accepts the session cookie or an API key and applies the file's visibility. A key that names `private` may read every private file of its Deployment, because the owner who issued it can already read them through the Admin Interface. There is still no unauthenticated object URL.

The signing secret is set by the owner through the Admin Interface and belongs to the Project, not to the Deployment, because a first publish must find it before the Deployment exists. It never enters `actions.json`, the Build Artifact or a portable export. Keys travel with the identity file in an export, hashes included, and the import revokes them, as it deletes sessions and challenges. The owner issues new keys on the destination.

Considered and rejected: letting visibility gate routes too, which would make a private app unable to receive a webhook; allowing `http` on `authenticated` actions with a cookie, which would put a person's rows behind a URL that cross-site requests can reach; storing secrets in the Deployment folder, which leaves the first publish with nowhere to look.

## Note, 2026-11-06: invitations answer through the same challenge

Delivery E plugs app invitations into this model rather than beside it. An invitation is an `invitation` challenge on a pending account, and `complete_challenge_with` in `auth.rs` answers it with the step verification and recovery use: consume the token, set the password, activate, issue the session. The policy check and the state check run inside that transaction. An active account is refused with "sign in instead", so an invitation never replaces a password, and a suspended one stays suspended. Inviting a new email creates the pending account at once, as signup in required mode does, so two answers to one link race on one row and one consumption.

The history lives in `app_invitations` beside the accounts, in `kernel-rs/src/invitations.rs`. Keeping the secret in `account_challenges` means the rule "a copy drops every challenge" already keeps links off other machines; the export and the import also clear the rows' token hashes, and a pending invitation shows `needs-resend` at the destination.

The admin tools that list accounts, suspend them, invite and change the signup policy need a new permission, `manage_users`. The owner's session holds it; delegated tokens hold it only when issued with it. `signup_policy_set` moved from `publish` to `manage_users`, so a publish credential can name the policy of a first release and cannot change who may join an existing app.

We considered letting `release_plan` change an existing app's policy when the credential holds `manage_users`. We kept one path for changes, `signup_policy_set`, because a republish that silently carries a policy is how a publish-only agent would widen access, and a second path is a second place to get that check wrong.

## Note, 2026-11-06: account email leaves through a queue

Account email now leaves through a persistent queue in `mail.sqlite`, recorded in [ADR-0012](0012-account-email-goes-through-a-persistent-queue.md). `AccountMail` stays as the transport seam the development mailbox and tests use. The identity store gained `request_signup_with` and `request_recovery_with`, which take a `ChallengeOutbox` that writes in the challenge's transaction.

## Note, 2026-11-06: API keys on restore and on import

A restore and an import treat API keys differently, and the difference follows who ends up holding the app. `littlebox-kernel restore` installs a copy of the same installation for the same owner, after a lost volume or a move to a new host. It signs out every session and agent credential because the owner can sign in again in a minute, but it keeps API keys valid, because they sit in programs and services the owner may not control and the app is still the same app. An import, through a package or the small export, puts the app in another installation, possibly under another owner, so it revokes every key: the package leaves the keys out and its import deletes any it finds, and the small format marks each one revoked.

A key the owner revoked after the copy was taken comes back live after a restore. The restore command says so and tells the operator to revoke it again. `a_restore_keeps_api_keys_live_and_an_import_revokes_them` in `kernel-rs/src/copies.rs` checks both paths on one accounts file, and `tests/http-routes.integration.test.ts` checks the import against a running Kernel.

## Note, 2026-11-06: hardening the HTTP update

Four changes to the HTTP update above, from the open items of delivery 7.

Accepted signatures persist. The memory of seen signatures was in the Kernel process, so a restart inside the 300 second window accepted a replay. It is now a `route_signatures` table in the Deployment's `identity.sqlite`, keyed by path and MAC and pruned past 600 seconds. We chose `identity.sqlite` over `usage.sqlite` because Usage has one background writer that drops entries when its queue fills, and a replay check must answer synchronously and never drop. `identity.sqlite` already holds the other caller proofs, sessions and keys. The table travels in an export, so the destination refuses a request captured on its way to the origin. A restore brings back the table as the copy held it, so a signature accepted after that copy can be replayed once more while its `t` is still within 300 seconds of the Kernel's clock.

`HEAD` on a `GET` route answers 405 with `Allow: GET`. axum answered `HEAD` with the `GET` handler, which ran the query and dropped the rows. Answering with the `GET` headers without the query would be guessing: the status depends on the caller, the rate limit and the input. `HEAD` on any other path still serves the artifact.

An unknown or revoked `lbk_` key counts against the client address, as a wrong password does: 10 a minute per Deployment, checked before the lookup and taken only on a miss. When they are used up, every key from that address answers `rate-limited` without a lookup, a valid key included, until the bucket refills. A valid key never drains it.

The Kernel parses a Deployment's routes once per release, keyed by the `artifact_version` each publish mints, and `deploy` replaces the parsed table when it installs a release. A static file request no longer parses `actions.json`.

## Note, 2026-11-06: revocations after the copy

A key revoked, or an account suspended, after the copy no longer comes back with a restore. Each revocation and suspension is written to the copy targets as it happens, and `restore` applies that record before the Kernel serves; [ADR-0014](0014-an-app-migrates-with-one-writer-at-a-time.md) records how. Keys that were live when the owner last saw them stay live, as above. A restore now also ends every open verification, recovery and invitation link, on both scopes.

## Note, 2026-11-06: an invitation is acceptable under Closed

Closed refused both a signup and the acceptance of an invitation. The gallery showcase app was Closed from its first release and could admit no one: the owner's invitation went out with `acceptable: false`, and the only way in was to open Invitation only for the minute it took to accept, then close again.

Closed now refuses only what a person does alone: a signup, and the verification that completes one. An invitation is the owner choosing one person for one address, so `admits_invitation` in `kernel-rs/src/invitations.rs` admits it under Open, Invitation only and Closed, and `app_invitations_create` answers `acceptable: true`. The acceptance still rechecks the policy and the account's state in its transaction. An Allowlist still has to list the address, because the list is a rule the owner wrote about addresses, and an invitation outside it answers `acceptable: false` as before.

One consequence follows. Closing signup no longer ends pending invitations; the owner revokes the ones that should end. We considered ending every pending invitation when the policy moves to Closed, and kept them, because that would send people a link that stops working without a word, and revoke already says it. `closed_refuses_a_signup_and_accepts_an_owner_invitation` in `invitations.rs` and the gallery replay check both sides.

## Update, 2026-11-06: a route binds its action

The showcase apps found that the HTTP update above protected a path and not the data or the action behind it. A member of a private inbox stored a webhook event through `actions_invoke` with no signature; a visitor of a public booking board checked in a booking the same way; an unsigned `GET` route on a private app handed its rows to anyone; and the MAC covered `t` and the body only, so a signature captured for one query fit any query and two requests in one second collided. This update replaces the rule "an action's access level decides whether its HTTP binding answers".

A route answers to the callers it takes. The binding may name them with `"callers"`, any of `"signature"`, `"key"` and `"session"`, each once; `"signature"` is listed exactly when the binding declares a signature. Without the list, a signed route takes the signature or a key, as before; an unsigned route on a private Deployment takes a key or a session; an unsigned route on a public Deployment answers anyone. A key the route does not take is refused, and a session it does not take leaves the person a visitor who must sign. A caller with no proof the route takes gets 401 `authentication-required`. The parser cannot see visibility, so the private-Deployment default is resolved when the request arrives; the declaration rules are parsed by both doors from `kernel-rs/src/http-binding-cases.json`.

An action whose route does not answer anyone is not callable through `actions_invoke`; its callers are the route's callers only. That is every signed route, every route with `callers`, and every unsigned route on a private Deployment. The runtime endpoint answers `Action <name> answers only on its HTTP route, <method> <path>.` before it authenticates. An unsigned route on a public Deployment keeps both doors, since both answer anyone. We considered letting a session through `actions_invoke` when `callers` lists `"session"`, and kept one door: two doors for one action are two places to get the check wrong, and the app's own page reaches the route with `fetch`, which sends the session cookie.

The signature now covers the method, the path and the canonical query string, and carries a nonce. The header is `t=<unix seconds>,n=<nonce>,v1=<hex>`, and the MAC is keyed with the secret over `t`, the nonce, the method, the path, the canonical query string and the raw body, joined by newlines. The canonical query string is the pairs sorted by key, then value, in `application/x-www-form-urlencoded`, so the sender's order and escaping do not matter. A retry is signed again with a new nonce instead of colliding with the first request. The replay table stays as the hardening note describes, keyed by path and MAC. A `t` outside the window answers `stale-timestamp`, checked before the MAC so the answer says nothing about any secret; a wrong MAC still answers `invalid-signature`. `src/lib/route-signature.ts` and `kernel-rs/src/http.rs` sign the vectors in `kernel-rs/src/signature-cases.json` to the same header. Senders written for the earlier scheme stop working: a header without `n=` is `invalid-signature`.

The runtime endpoint refuses a request that carries an API key with 403 and says where a key works, instead of reading it as an unknown session token and answering as for a visitor.

## Note, 2026-11-07: findings of the security review

- An account holds at most 20 open sessions, and a sign-in past that ends its oldest. Sign-in is rate limited but always succeeds with the right password, so before this one account could fill the 64 MiB identity file from three addresses in a week and stop every sign-in on its app.
- Creator signup in local mode answered 409 for a taken email before it looked at the code, so a made-up code told whether a creator existed. The admission now decides first: without a valid code, a taken email and a new one get the same 403. A valid code still learns that its email is taken, and keeps the code.
- A route signature header may carry at most two `v1=` values of 32 bytes, one for each secret during a rotation. Each value cost one MAC over the body, so a header with thousands of empty values cost seconds of CPU per request.
- The file read path sends a JavaScript, HTML, XML or SVG type as `application/octet-stream`. `Content-Disposition: attachment` and the file's `sandbox` policy apply when a file is opened as a page, not when a page on the app's origin loads it with `<script src>` or `new Worker`, which `script-src 'self'` allows. A Deployment holds at most 10,000 files and a name loses its control characters, so its file index stays below the size a package import accepts.
- A route signature covers the method, the path, the query string and the body, but not the host, so the review replayed a signed request captured for one Deployment on a second Deployment of the same owner that held the same secret value, inside the 300-second window. Binding the host would change the wire format for every sender, and senders behind a proxy do not always know the host the Kernel sees. `secret_set` instead refuses a value another project of the same owner holds. Two owners who choose the same value by chance are not checked, because the refusal would tell one owner another's secret. An app moved to another installation still accepts a request signed for its origin while both hold the same secret; that is the same app.

## Note, 2026-11-07: a key reads what a visitor reads

The starter app found that a key naming no file visibility answered 404 for a public avatar that a request with no credential read. A key now reads a public file of a public Deployment as a visitor does, and the visibilities it names on top. On a private Deployment a visitor reads nothing, so there the key reads only what it names, and `files: ["public"]` keeps its meaning. The check is `key_reads` in `kernel-rs/src/http.rs`.
