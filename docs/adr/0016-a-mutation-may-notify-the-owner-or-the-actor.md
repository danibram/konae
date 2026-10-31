# A mutation may notify the owner or the actor

Status: accepted, 2026-11-07. Amended by [ADR-0021](0021-a-notice-lists-its-rows-and-reaches-the-accounts-they-name.md), which adds `{kernel.now}`, `{changes}` and `to: { column }`. Builds on [ADR-0012](0012-account-email-goes-through-a-persistent-queue.md), whose queue carries the email, and [ADR-0010](0010-kernel-owns-usage-and-signals.md), whose Usage store counts it. Amends ADR-0012's rule that no text from a creator reaches a message.

The capability matrix ([docs/research/capability-matrix.md](../research/capability-matrix.md)) found email on an event missing from seven of the ten app shapes people ask for: a waitlist that tells the owner someone joined, a form collector that announces a submission, a booking board that confirms a booking, an alert inbox that pages its owner. The queue could only send account links. The nearest alternative was a program the owner runs that polls a keyed route and sends the mail itself, which a person who does not write code cannot set up, and which does not leave with the app.

## Decision

A mutation may declare `"notify": { "to": "owner" | "actor", "subject": "<template>", "body": "<template>" }`. The Kernel reads it from `actions.json` like a guard or a Signal and acts on it inside its one process. No user code runs.

**Who receives it.** `owner` is the email of the Project Owner's active account in `owners.sqlite`. `actor` is the email of the calling app account, and only when that account is active and has `verified_at` set, which answering a verification, invitation or recovery link does. The Kernel never sends to an address from the input, and `to` takes no placeholder, so an app cannot become a relay to whoever a visitor names. Verification is the actor's rule and not the owner's: a stranger can sign up with someone else's address under Open signup in local mode and make the app write to it, while the owner was admitted by the setup key, an invitation code or Clerk. An owner who signs in through Clerk has no email in the Kernel, and the notice is refused with `owner-has-no-email`. A visitor, a signed request and an API key have no account, and `to: actor` is refused with `no-account`.

**What it says.** The subject and body are templates written by the app's author. Their only placeholders are `{input.<key>}` for one of the action's input bindings and `{actor.id}`; `{actor.email}` and anything else are refused at both doors, so a template cannot read what the bindings cannot. A value loses its control characters and line breaks, which keeps it out of the headers, and keeps 200 characters. The subject is one line of at most 120 characters as written and 200 rendered; the body is at most 2,000 characters as written and 5,000 rendered, and may keep its own line breaks. The HTML part escapes everything. The Kernel adds a last line naming the app and why the recipient gets it.

**When it is sent.** Only when the statement changed a row, the same moment a Signal is recorded: a guard's refusal, a retry that stores nothing and a failed statement send nothing. The notice is recorded after the answer is decided and never changes it.

**How it leaves.** As a job in ADR-0012's queue with a new purpose, `notify`. The `jobs` table's `CHECK` on the purpose cannot be altered in SQLite, so the Kernel rebuilds the table, keeping its rows, the first time it opens a queue created before this decision. The rendered subject, text and HTML are sealed in the job's link columns with the queue's key and the job ID as associated data, because they carry the app's input, and are dropped when the provider accepts the job. A notice has no challenge to check, so instead it fails as `notice-expired` when it has not left within a day; a confirmation a day late is worse than none. A paused app holds its notices like its account email. Restore ends them like any pending job.

**Budgets.** Notices count apart from account email, so a busy app can never use up the allowance that signup and recovery need: 200 a day per app, 500 a day per creator, and 50 a day per recipient from one app. The installation's 1,000 a day counts every email, because that limit protects the sender's reputation with the provider. A notice over a budget stores nothing, so a hot action cannot grow `mail.sqlite` without bound. The values are `KONAE_MAIL_LIMIT_NOTIFY_*`.

**When the sender is not ready.** While the sender preflight is not `ready`, a notice within its budgets is stored as a failed job with the kind `sender-not-ready` and no text, and is never sent later: the owner reads why, and nobody gets a stale confirmation when the sender comes back. Those rows count toward the budgets, which bounds them.

**What the owner reads.** Usage counts every notice under the kind `notify`, named `<action>: queued` or `<action>: <reason>`. `mail_status` reports `queue.notices`, the notice jobs per state, and a last failure whose `purpose` is `notify`. It names no recipient.

**Both doors.** `parseNotify` in `src/kernel/actions.ts` and `notify::declaration` make the same checks in the same order with the same messages, from `kernel-rs/src/notify-cases.json`. The editor preview sends no email.

## Considered options

- **Let the input name the recipient.** Rejected. It turns every public app into a relay, and no budget fixes that.
- **Send to every member, or to account IDs a query returns.** Deferred. It is the form a community or a two-sided app needs for followers, and it multiplies one call into many emails, so it needs its own budget and an unsubscribe. The matrix keeps those cells at C.
- **Count notices under the existing app and creator budgets of 100 a day**, as the showcase findings proposed. Rejected. A hundred notices would stop signup and recovery on that app for a day, and a per-app notice budget of 200 could never be reached.
- **Fail the mutation when the notice cannot be sent.** Rejected. The row already changed, and the person who booked a room should not see an error because the installation's sender is down.
- **Keep the sender-not-ready notice and send it once the preflight passes.** Rejected for the staleness above. The owner sees the failed job and the Usage line instead.
- **Let the template run code or read rows.** Rejected by the product rule: no user code on the server.

## Consequences

Text written by an app's author now reaches an email, within the limits above. Email on an event works for shapes that tell the owner (a waitlist, a form, an integration hub) and the person who acted (a booking or an order). It does not reach a third person, such as an assignee or a follower. In local mode an actor proves an email only by answering an invitation or recovery link, so the showcase booking replay invites its booker. The queue's `jobs` table gains an `action` column. A notice is part of the installation's queue, so it does not travel in a package or an Eject, and an imported app sends none of its origin's pending notices.
