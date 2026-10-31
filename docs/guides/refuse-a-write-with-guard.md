# Refuse a write with guard

Put the app's rule in a mutation's `WHERE`, add a `guard` sentence, and the caller gets that sentence when the rule says no.

## How it works

When a mutation's statement changes no row, the Kernel normally answers `{ changes: 0, lastInsertId: 0 }`, the same answer as a call that had nothing to do. A `guard` turns that answer into a refusal. It is a tool error for `miniapp.actions.call` and a 403 on the action's HTTP route. Usage counts it under `refused` and no Signal is recorded.

A guard is 1 to 200 characters on one line. It goes on a mutation only. A query or a signal action that declares one is refused. A scheduled run has no caller, so a guard does not apply to it.

## Steps

1. Write the rule into the statement, so the statement changes no row when the rule fails. This example changes a row only for the member who owns it, and only when the name is free.
2. Add `guard` next to `kind` and `access`. The sentence is what the person sees, so write it for them.

    ```json
    "me.rename": {
      "kind": "mutation",
      "access": "authenticated",
      "guard": "Another member already uses that display name, or it is yours already.",
      "statement": "UPDATE profiles SET display_name = ?2 WHERE user_id = ?1 AND display_name IS NOT ?2 AND NOT EXISTS (SELECT 1 FROM profiles other WHERE lower(other.display_name) = lower(?2) AND other.user_id <> ?1)",
      "bindings": [
        { "source": "actor", "field": "id" },
        { "source": "input", "key": "displayName", "type": "string", "maxLength": 40 }
      ]
    }
    ```

3. For an owner-only action, keep the role in the app's own table and test it in the `WHERE` against the caller's ID. The Project Owner names the app's owner with SQL in Data.

    ```json
    "members.promote": {
      "kind": "mutation",
      "access": "authenticated",
      "guard": "Only the app's owner can make a member a team admin.",
      "statement": "UPDATE profiles SET role = 'admin' WHERE user_id = ?1 AND role = 'member' AND (SELECT role FROM profiles WHERE user_id = ?2) = 'owner'",
      "bindings": [
        { "source": "input", "key": "userId", "type": "string", "maxLength": 64 },
        { "source": "actor", "field": "id" }
      ]
    }
    ```

4. Show the refusal in the page. `miniapp.actions.call` rejects with the guard sentence as `error.message`.

    ```js
    try {
      await miniapp.actions.call("me.rename", { displayName });
    } catch (error) {
      status.textContent = error.message;
    }
    ```

5. Do not add a second check statement. The statement has already run and changed nothing, so nothing rolls back.

## Check it worked

- Call the action twice through the same door the page uses, once with input the rule accepts and once with input it refuses. The first answers `{ changes, lastInsertId }`. The second answers your sentence. `actions_invoke` is its own read-back.
- Read Usage with `usage_read` and find the call under `refused`.
- After a release, the read-back for `release_apply` is `project_inspect`.

The editor preview refuses with the guard the same way a Deployment does, so you can try it there. It has no Usage store, so read `usage_read` on `konae dev` or a Deployment.

## Read next

- [Add a Server Action](./add-a-server-action.md)
- [Email when a row changes](./email-when-a-row-changes.md)
- [Accounts and access](../accounts-and-access.md)
- [Server Actions](../server-actions.md)
