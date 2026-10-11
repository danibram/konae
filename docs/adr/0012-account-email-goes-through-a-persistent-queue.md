# Account email goes through a persistent queue

Status: accepted, 2026-11-06. Builds on [ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md).

ADR-0011 made verification and recovery depend on email and left the transport as a seam with a development mailbox. The launch plan needs a real inbox, a provider outage that loses nothing, no email claimed as delivered that was not, and limits that stop Littlebox from becoming an email relay. Delivery B of the [launch plan](../plans/platform-launch.md) builds that.

## Decision

Account email leaves through a queue in its own SQLite file, `mail.sqlite`, in the data directory. One installation has one queue; it is not part of any app.

**One transaction for a challenge and its job.** The identity connection attaches `mail.sqlite` before it begins the transaction that issues a challenge, and `JobOutbox::record` writes the job there before the commit. A refusal inside `record`, such as a limit reached by a concurrent request, rolls the challenge back too. With the identity database in WAL mode, SQLite commits each attached file atomically on its own, not the pair: a power loss inside the commit could keep the job and lose the challenge. The worker therefore sends a job only while its challenge is still open in the identity database. That rule also covers a link replaced by a newer one, a challenge ended by suspension, and one that expired before sending. A job younger than a minute whose challenge is not visible yet waits instead of failing, because the second file's commit may still be landing.

**What a job holds.** Purpose, recipient, scope (`creator` or `deployment:<id>`), the creator who owns the app for the per-creator limit, the app name, the challenge's token hash, the link sealed with AES-256-GCM, the link's expiry, state, attempts, next attempt time, a lease and the provider's message ID. The job ID is the idempotency key. The visible states are the plan's four: pending, accepted by the provider, delivered and failed. A failed job carries a kind that says why.

**The sealing key.** HKDF-SHA256 derives an AES-256-GCM key from `LITTLEBOX_MAIL_SECRET`, or from `LITTLEBOX_ADMIN_KEY` when that is unset. Every installation reachable from another machine already holds a secret admin key, so the fallback adds no step for the operator, while a separate secret lets the operator rotate the admin key without losing the queue. The development default admin key is public, so the Kernel refuses to start `resend` or `smtp` with it and no separate secret; the development mailbox writes its links in clear to `mailbox.jsonl` anyway and may use it. The job ID is the associated data, so a sealed link copied to another row does not open. The link is deleted when the provider accepts the email or the job fails.

**The worker.** A Tokio task drains the queue on an interval and when a job is enqueued; no request waits for a provider. A pass claims due jobs with a two-minute lease and counts the attempt. A Kernel stopped mid-send leaves the lease to expire and the next pass repeats the request with the same idempotency key, which Resend answers with the same email. Retries wait 30 seconds, doubling up to an hour, for at most nine attempts, about two hours in all, inside Resend's 24-hour key window.

**Transports.** Resend over HTTPS with `reqwest` 0.12.28 on rustls, which shares the `hyper` 1 and `hyper-rustls` 0.27 already in the tree through the S3 client. SMTP with `lettre` 0.11.23, pinned, the maintained Rust SMTP client: async on Tokio, STARTTLS or implicit TLS through rustls with bundled web roots, no OpenSSL. The development mailbox stays as it was. `LITTLEBOX_MAIL_TRANSPORT` chooses one; unset keeps the 503.

**Sender preflight.** At startup, every 10 minutes while refused, and on demand through `mail_status`, the Kernel checks the sender. For Resend the sender's domain must be `verified`, read with `GET /domains`. A sending-only key cannot read domains, and the check refuses rather than assume; `LITTLEBOX_RESEND_DOMAINS_KEY` supplies a key that can. A 401 or 403 from Resend at send time also turns sending off. For SMTP the check connects, negotiates TLS and authenticates. While the check refuses, signup and recovery answer 503 and nothing is queued.

**Provider events.** Resend webhooks reach `/_littlebox/mail/resend-events`, signed with Svix. The Kernel verifies the HMAC, records each event ID once, and moves a job only forward; the first final state wins. The handler touches `mail.sqlite` only, so no event activates an account.

**Limits.** Before anything is stored, the queue checks the recipient (3 a minute, 10 a day), the client address (20 an hour), the creator (100 a day), the app (100 a day) and the installation (1,000 a day). The recipient and client limits count requests, recorded as hashes, whether or not a message follows, so the answer never depends on whether an account exists. The creator, app and installation limits count jobs and are checked again inside the challenge's transaction. The plan gave no per-app or per-client value; these are ours.

**Templates.** The Kernel renders verification, recovery and invitation from fixed text. Only the app name, escaped for HTML and stripped of line breaks, and the link, built from the Deployment's validated origin or the editor's, vary. No text from an agent or a creator reaches a message.

## Considered options

- **Keep the job in the identity database as an outbox table.** Rejected: identity databases travel in exports, and the plan excludes tokens and unsent jobs from them. The worker would also scan every Deployment's database.
- **Write the job after the challenge commits.** Rejected: a crash between the two leaves a challenge whose link nobody receives, and the person sees an answer that says a link is on its way.
- **Count every limit on jobs only.** Rejected: an active or suspended address gets no job, so the eleventh request would answer differently for an address with an account than for one without.
- **Trust a sending-only Resend key and rely on the 403 at send time.** Kept as a second line, rejected as the check: the first real signup would be the test, and its job would wait for an operator.
- **A fifth `cancelled` state.** Rejected to keep the plan's four states; a failed job's kind says it was superseded or expired.

## Consequences

`mail.sqlite` is in scheduled copies with its links sealed. A restore ends its pending jobs, because a restore invalidates the challenges they carry, and an app export does not include the queue, so a migrated app never replays its origin's email. The identity transaction begins with `BEGIN IMMEDIATE` while `mail.sqlite` is attached, so it holds the queue file's write lock too: signups and recoveries across every app serialize on that file for the length of one identity transaction, a few milliseconds today. That is the scaling boundary of this design. SMTP has no idempotency key: a Kernel stopped between an SMTP server's answer and the queue's record sends that email twice. Rotating `LITTLEBOX_MAIL_SECRET`, or the admin key it falls back to, fails the pending jobs and people ask for a new link. Invitations (delivery E) and the MCP user tools (delivery F) enqueue through the same outbox.

## Update, 2026-11-06: the invitation names its inviter

An invitation email now names the owner who invited, by the email of the owner's account in `owners.sqlite`, next to the app name: `owner@example.com invited you to Tasks`. The value is the account record's email, which the Kernel normalized at signup, not text from the tool call, so the rule above holds: no text from an agent reaches a message. It gets the app name's treatment, line breaks removed and HTML escaped. `invitations.rs` reads it when it hands the challenge to the outbox; the job keeps it in a new nullable `inviter` column, which `install` adds to a queue created before it, so the worker renders it later without reading the owner store. An owner who signs in through Clerk has no email in the Kernel, and that invitation names only the app. Verification and recovery messages are unchanged.

## Note, 2026-11-07: the day limit per address counts per scope

The security review found that ten recovery requests for an address on any app, each answered 202 and none sent because the address had no account there, used up that address's day for every app and for the editor, so the person could not recover an account anywhere for 24 hours. The day limit now counts an address per scope (`creator` or one Deployment), and the minute limit still counts it across the installation, so a burst across many apps still stops after three. The client limit counts an IPv6 client by its /64, as every other limiter does; it counted each address before. `normalize_email` refuses `<>()[],;:\"` and control characters, which the mail libraries read as a display name, a comment or a list, so `x<victim@example.com>` no longer counts as a new address with its own allowance and reaches the same inbox.

## Update, 2026-11-07: the queue carries notices

[ADR-0016](0016-a-mutation-may-notify-the-owner-or-the-actor.md) adds a fifth purpose, `notify`: an email a mutation declares, sent to the Project Owner or to the calling account's verified email when the statement changes a row. Its text comes from the app author's template with input values escaped and stripped of line breaks, so the rule under Templates, that no text from a creator reaches a message, now holds for account email only. The rebuild that adds the purpose keeps every row. The creator and app limits above count account email only; notices have their own per-app, per-creator and per-recipient budgets, and the installation limit counts both. A notice has no challenge, so it fails as `notice-expired` after a day instead of being checked against one, and while the preflight refuses, a notice is stored as a failed job with the kind `sender-not-ready` instead of being refused with 503.
