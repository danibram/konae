# Rehearse a source change locally

Rehearsal checks current sources against a fresh copy of published data. It keeps edits to local rows out of the result and does not change the Deployment.

## Refresh the data snapshot

Pull only data to keep local sources and their recorded baseline:

```sh
bun run konae -- pull --dir ./my-app --data-only
```

The snapshot is stored in `.konae/pull/`. It is immutable for a rehearsal. The pull does not replace local `app/`, settings, or source baseline.

## Reset and run the local app

Stop any running `dev` process, then reset local data from the snapshot:

```sh
bun run konae -- dev --dir ./my-app --reset
```

Reset takes a lifecycle lock, backs up `data/` and the local call and mail queues, imports the snapshot, then retires unfinished local effects. Its output gives the execution and snapshot IDs, backup path, and retired counts. If it fails, reset restores the saved data and queues. Start `dev` without `--reset` to keep working with that local data.

The local Kernel binds to loopback and uses its own accounts, rows and files. Email goes to `.konae/kernel/mail-testbox.jsonl`. Call actions are captured without DNS, secret-based authentication, app dispatch or HTTP. Schedule timers pause; run a declared schedule manually with `konae test schedule`. Google sign-in is refused. Browser connections to external origins are blocked. This does not sandbox npm build scripts or other shell commands.

The events page printed by `dev` has a random control token in its URL fragment. It checks the loopback host and request origin, and requires that token in control requests. The CLI also exposes `konae test status`, `calls`, `reply`, `schedule` and `scenario`; see the [CLI reference](../cli.md#test).

Captured calls show their attempt, response number and retry time. Supply one synthetic HTTP response or transport error with `test reply`; the Kernel applies its normal retry rules. A continuation marked `interrupted` has an unknown outcome and will not replay after restart. Reset local data and run the workflow again before relying on it.

## Check the candidate

Rehearse uses current sources and a fresh build with the snapshot, in a scratch folder:

```sh
bun run konae -- rehearse --dir ./my-app
```

It checks that the candidate can import the snapshot, reports digests, and removes scratch state. It ignores hand-fixed local `data/`. If the candidate schema needs a confirmation, the refusal prints the value to pass on a second run:

```sh
bun run konae -- rehearse --dir ./my-app --confirm-schema <confirmation>
```

The first schema refusal prints a confirmation for this snapshot's scratch import. Pass it to rehearse as shown above. The later production plan uses current Deployment rows and may print a different value. `konae schema fingerprint --dir ./my-app` prints `sha256:<64 lowercase hex characters>` for the schema; add `--data` to read the local database read-only, or `--json` for structured output. The Kernel command accepts `--schema <path>` or `--database <path>`. Fingerprinting starts no app and creates no `.konae/`.

## Plan and publish

Review the release against the current Deployment before applying it:

```sh
bun run konae -- publish --dir ./my-app --plan
bun run konae -- publish --dir ./my-app
```

Publish changes sources and the built artifact against current production rows. It never uploads local rows. If the production plan requests a schema confirmation, pass that plan's value to `publish --confirm-schema`. `konae push --yes` takes a backup, then replaces published data with local `data/`. Use it when you intend to replace those rows.
