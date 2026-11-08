# Email leaves through one queue, to addresses the app has proved

Status: accepted.

Verification, recovery and invitations depend on email, and apps want to email on an event: a waitlist that tells its owner someone joined, a booking that confirms itself, a reminder that reaches the person it names. A provider outage must lose nothing, no email may be called delivered that was not, and no app may become a relay to whatever address a visitor types.

## Decision

### One persistent queue

Every email leaves through one queue in `mail.sqlite`, at the root of the data directory. It belongs to the installation, not to any app, and an export does not carry it.

- The transaction that issues a challenge attaches `mail.sqlite` and writes the job before it commits, so a refusal rolls both back. The worker sends a job only while its challenge is still open, which also covers a link replaced by a newer one, a challenge ended by a suspension and one that expired.
- A job holds its purpose, recipient, scope, the app name, the link sealed with AES-256-GCM and the job's state. The states are pending, accepted by the provider, delivered and failed, and a failed job carries a reason. The job ID is the idempotency key and the associated data of the seal. The sealed text is deleted when the provider accepts the email or the job fails.
- HKDF-SHA256 derives the sealing key from `KONAE_MAIL_SECRET`, or from `KONAE_ADMIN_KEY` when that is unset. The Kernel refuses to send through a real provider with the public development key. The `kernel` binary writes a random key on its first start, readable by its owner alone.
- One worker task drains the queue, so no request waits for a provider. A pass leases due jobs for two minutes. Retries wait 30 seconds, doubling up to an hour, for at most nine attempts.
- `KONAE_MAIL_TRANSPORT` chooses Resend over HTTPS, SMTP, or the development mailbox, a file. Without one, signup and recovery answer 503.
- At startup, every 10 minutes while refused, and on demand through `mail_status`, the Kernel checks the sender. Resend needs a verified domain, and SMTP needs a TLS connection and a login. While the check refuses, signup and recovery answer 503 and queue nothing.
- Resend's events reach `/_konae/mail/resend-events`, signed with Svix. The Kernel records each event once and moves a job only forward.
- While an app is in maintenance, the worker holds its email.

### Limits come before anything is stored

Account email is limited per recipient (3 a minute, 10 a day per scope), per client address (20 an hour, an IPv6 client by its /64), per owner (100 a day), per app (100 a day) and per installation (1,000 a day). The recipient and client limits count requests, whether or not a message follows, so the answer never depends on whether an account exists. An address with `<>()[],;:\"` or a control character is refused, so a display name cannot pose as a new address.

Notices have their own budgets, 200 a day per app, 500 per owner and 50 a day per recipient from one app, so a busy app never uses up the allowance that signup and recovery need. The installation's 1,000 a day counts both. The values are `KONAE_MAIL_LIMIT_*` settings. The owner is the account that owns the app on Konae, or the operator on a Kernel without Konae.

### Account email uses fixed text

The Kernel renders verification, recovery and invitation from fixed templates. Only the app name, the link and, in an invitation, the inviting owner's email vary. Each loses its line breaks and is escaped for HTML.

### A mutation may send a notice

A mutation may declare `"notify": { "to": ..., "subject": "<template>", "body": "<template>" }`. The Kernel reads it from `actions.json` and runs no code of the app's.

- `to: "owner"` sends to the app's contact email in `konae.json`. The owner sets it in App settings or with `contact_email_set`, and a first publish from Konae fills it with the owner's email when Konae knows it. The editor says when an app declares an owner notice and has no contact email.
- `to: "actor"` sends to the calling account, only when it is active and its email is verified.
- `to: { "column": "<name>" }` sends to each account whose ID a changed row holds in that column, among this app's active, verified accounts. A value that is an email address, or the ID of an unverified, suspended or missing account, sends nothing. One run reaches at most 50 accounts, in account ID order.
- The templates take `{input.<key>}`, `{actor.id}` and `{kernel.now}`. A notice that names `"changes": [<columns>]` may use `{changes}` in its body, one line per changed row with those columns, and `{changes.count}`. Each recipient of a column notice sees only the rows that name them. The Kernel reads the rows by appending its own `RETURNING` to the statement in the same transaction, and the rows never reach the caller's answer.
- A value loses its control characters and line breaks and keeps 200 characters. The subject is one line. The HTML part escapes everything. The Kernel adds a last line naming the app and why the recipient gets it.
- A notice is queued only when the statement changed a row, after the answer is decided. It never changes the answer. A notice that cannot be queued counts in Usage under `notify` with its reason, such as `owner-has-no-email` or `not-a-verified-account`.
- While the sender check refuses, a notice is stored as failed with the reason `sender-not-ready` and never sent later. A notice that has not left within a day fails as `notice-expired`.

## Rejected options

- **Keep jobs in each app's identity file.** Identity files travel in exports, which must carry no tokens or unsent jobs, and the worker would scan every app.
- **Write the job after the challenge commits.** A crash between the two leaves a link nobody receives, after an answer that said one is on its way.
- **Count every limit on jobs only.** An address with an account gets no job, so the eleventh request would answer differently for an address with an account.
- **Let the input or a row name an address.** Every public app would become a relay, and no budget fixes that.
- **Count notices under the account email budgets.** A hundred notices would stop signup and recovery on that app for a day.
- **Fail the mutation when its notice cannot be sent.** The row already changed, and the person who booked a room should not see an error because the sender is down.
- **Send a held notice once the sender is ready.** A confirmation a day late is worse than none.
- **A second statement that names the recipients.** It cannot tell which rows the mutation changed, and a DELETE leaves nothing to query. `RETURNING` reports exactly the changed rows in the same transaction.
- **Let the author write `RETURNING`, or use `RETURNING *`.** A returned row would reach the caller, and `*` would mail columns the app never shows.

## Consequences

- Signups and recoveries across every app take the queue file's write lock for the length of one identity transaction. That is where this design stops scaling.
- SMTP has no idempotency key, so a Kernel stopped between the server's answer and the queue's record sends that email twice.
- Rotating `KONAE_MAIL_SECRET`, or the key it falls back to, fails pending jobs, and people ask for a new link.
- A restore ends pending jobs, and an imported app sends none of its origin's email.
- There is no per-recipient unsubscribe yet. Verification proves an address belongs to an account of the app, not consent to every notice, and the per-recipient budget bounds the harm.
- The editor preview sends no email. `konae dev` writes it to the development mailbox file, and `konae events` lists it.
