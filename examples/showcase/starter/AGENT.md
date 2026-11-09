# Adapting Starter

Rename tables and actions freely, but keep these rules:

1. Every row a person owns is written and read with `{ "source": "actor", "field": "id" }`, never with an input that names a user.
2. Every role or ownership check sits in the mutation's `WHERE` and declares a `guard` sentence the page shows. A query hides rows with `EXISTS (...)` on the caller's role; it cannot refuse.
3. The app's owner is the `profiles` row whose role is `'owner'`. Check it against `{ "source": "actor", "field": "id" }`. The Konae owner sets that row with SQL in the editor's Data panel. Never appoint an owner through a route, an input or a row a member can write.
4. Every query ends with a `LIMIT`. Page with a nullable `before` input, search with a nullable `q`.
5. A route on a public app answers anyone unless it declares `"callers"`. Write routes say `["key"]`; webhooks declare a `signature`, and the owner runs `secret_set` before each first release.
6. Alias every computed column. `schema.sql` holds only tables and indexes.
7. Upload private by default. Publishing or deleting a file is the file call, then the row's action.
8. Put `signal` on mutations that matter, with `actor`. Signals fire only when a row changed.
9. Never change roles with `database_execute`; use a guarded action.
10. Build elements with `textContent` and `addEventListener`; load nothing from another origin.
11. Reach another app with a call action, never with `fetch` and a secret in the page. Point `status.escalate`'s `url` at your inbox, set `inbox_events` with `"use": "call"`, and keep fields the page must not change as `{ "value": ... }` constants.

Then run `konae contract` and the replay.
