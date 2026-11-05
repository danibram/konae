# Email when a row changes

Add `notify` to a mutation and the Kernel queues a notice when its statement changes a row.

## How it works

A notice goes to the Project Owner, to the calling account, or to the accounts a changed row names. It never goes to an address from the input or from a row. The mutation's answer does not depend on it. Subject and body are templates that take only `{input.<key>}` for one of the action's input bindings, `{actor.id}`, `{kernel.now}`, `{changes.count}` and, in the body, `{changes}`.

## Steps

1. Pick the recipient. `"owner"` is the Project Owner's account. `"actor"` is the calling app account, and only once that account has proved its email with a verification, invitation or recovery link. `{ "column": "<name>" }` sends one notice to each verified account whose ID a changed row holds in that column.
2. Add `notify` to a mutation. This one emails the owner when someone joins a waitlist.

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

3. To email the person who called, use `"to": "actor"`. This one confirms an order.

    ```json
    "notify": {
      "to": "actor",
      "subject": "Your order for {input.print}",
      "body": "We have your order for {input.print}.\nYour account is {actor.id}."
    }
    ```

4. Keep within the limits. The subject is 1 to 120 characters on one line. The body is 1 to 2000 characters, with line breaks and no other control characters. `notify` belongs on a mutation, not on a query or a signal action. An address or `{input.email}` as `to` is refused.
5. Make sure the installation can send. Call `mail_status` and look for `sending` true. Without a mail transport nothing is stored and Usage records `no-transport`.

## When no email goes out

A notice is recorded only when the statement changed a row. A guard's refusal, an `INSERT ... ON CONFLICT DO NOTHING` that stored nothing and a failed statement send nothing. Usage counts each notice under `notify` with a reason such as `queued`, `no-account` (`actor` with a visitor or a key) or `unverified-email`.

## Check it worked

- Call the mutation, then read the notice's state with `notices_read` and the Deployment's `deploymentId`. It lists the app's notice jobs, newest first, 50 at a time.
- `mail_status` counts notice jobs per state under `queue.notices`.
- On `konae dev`, the email lands in `<folder>/.konae/kernel/mailbox.jsonl` and goes to no provider. `konae events` lists it with its recipient, subject and text, and `dev` serves the same list as a page on the next port.
- After a release, the read-back for `release_apply` is `project_inspect`. Delivered means a mail server took the message, not that a person read it.

The editor preview sends no notice email and has no Usage store. Try `notify` on `konae dev` or on a Deployment.

## Read next

- [Run work on a schedule](./run-work-on-a-schedule.md)
- [Invite people](./invite-people.md)
- [Server Actions](../server-actions.md)
- [Pick the smallest piece](../pieces.md)
