# Email when a row changes

Add `notify` to a mutation, and the Kernel queues a notice whenever its statement changes a row.

## How it works

A notice goes to the app's contact email, to the calling app user, or to the app users a changed row names. It never goes to an address from the input or from a row. The mutation answers the same whether or not the notice goes out. The subject and body are templates. They take only `{input.<key>}` for one of the action's input bindings, `{actor.id}`, `{kernel.now}`, `{changes.count}` and, in the body, `{changes}`.

## Steps

1. Pick the recipient.
    - `"owner"` sends to the app's contact email, `contactEmail` in `konae.json`. Set it in App settings or with `contact_email_set`. A first publish from Konae fills it with the owner's email.
    - `"actor"` sends to the calling app user, once that account has proved its email with a verification, invitation or recovery link.
    - `{ "column": "<name>" }` sends one notice to each verified app user whose ID a changed row holds in that column.
2. Add `notify` to a mutation. This one emails the contact email when someone joins a waitlist.

    ```json
    "waitlist.join": {
      "kind": "mutation",
      "access": "anonymous",
      "statement": "INSERT INTO waitlist (email, source, joined_at) VALUES (?1, ?2, ?3) ON CONFLICT (email) DO NOTHING",
      "bindings": [
        { "source": "input", "key": "email", "type": "string", "maxLength": 254 },
        { "source": "input", "key": "source", "type": "string", "maxLength": 40 },
        { "source": "kernel", "field": "now" }
      ],
      "notify": {
        "to": "owner",
        "subject": "New on the waitlist from {input.source}",
        "body": "{input.email} joined the waitlist.\nThey came from {input.source}."
      }
    }
    ```

3. To email the person who called, use `"to": "actor"`. This one confirms an order, on a mutation with a `print` input.

    ```json
    "notify": {
      "to": "actor",
      "subject": "Your order for {input.print}",
      "body": "We have your order for {input.print}.\nYour account is {actor.id}."
    }
    ```

4. To list the changed rows in the body, name up to 8 of their columns in `"changes"` and write `{changes}` in the body. The subject takes `{changes.count}` only.
5. Keep within the limits. The subject is 1 to 120 characters on one line. The body is 1 to 2000 characters, with line breaks and no other control characters. `notify` belongs on a mutation, never on a query or a signal action. An address or `{input.email}` as `to` is refused.
6. Make sure the installation can send. Call `mail_status` and look for `sending: true`. Without a mail transport the Kernel queues nothing and Usage records `no-transport`.

## When no email goes out

The Kernel queues a notice only when the statement changed a row. A guard's refusal, an `INSERT ... ON CONFLICT DO NOTHING` that stored nothing and a failed statement send nothing. Usage counts each notice under `notify` with a reason: `queued`, or one such as `owner-has-no-email` (no contact email), `no-account` (`actor` with a visitor, a key or a scheduled run) or `unverified-email`.

## Check it worked

- Call the mutation, then read the notice's state with `notices_read` and the Deployment's `deploymentId`. It lists the app's notices, newest first, 50 at a time. `mail_status` counts them per state under `queue.notices`.
- On `konae dev`, the email lands in `<folder>/.konae/kernel/mailbox.jsonl` and goes to no provider. `bun run konae -- events --dir <folder>` lists it with its recipient, subject and text, and `dev` serves the same list as a page on the next port.
- On `bun run dev:local`, the email lands in `data/kernel-local/mailbox.jsonl`.
- After `release_apply`, read the Deployment back with `project_inspect`. Delivered means a mail server took the message, not that a person read it.

The editor preview sends no notice and has no Usage store. Try `notify` on `konae dev` or on a Deployment.

## Read next

- [Run work on a schedule](./run-work-on-a-schedule.md)
- [Invite people](./invite-people.md)
- [Server Actions](../server-actions.md)
- [Pick the smallest piece](../pieces.md)
