# The Kernel owns Usage and Signals

Status: accepted.

An owner needs to see whether a Deployment is being used, and a Capsule needs a way to name a business occurrence without turning the browser into an analytics client. The Kernel already authorizes every login, action and upload. Recording those facts belongs with that authority.

## Decision

The Kernel keeps Usage and Signals in a per-Deployment store of its own, next to the app database and the identity file. They do not go in `audit`, and they do not go in the owner's tables. Usage is always on. The Kernel keeps daily totals for authorized Runtime Interface operations. A Signal is recorded only when a Server Action declared it and that action succeeds. Properties come from the action's bindings. The Project Owner reads both through the Admin Interface. A Runtime Actor does not. There is no `miniapp.track`. A Signal action is still `miniapp.actions.call`.

`audit` in `mcp.sqlite` stays the owner-tool log. App users already cannot grow it at will. Hanging Usage there would reopen that hole on a shared file. Writing Signals into `schema.ts` tables would make the host decide who may read a row, and would mix Kernel facts with the Database Model. A public dashboard still uses a Server Action and SQL, as the poll example does.

## Considered options

- **Append to `audit`.** Rejected because runtime traffic would share the owner log and a visitor could fill it.
- **Insert into the app database.** Rejected because the Database Model is the owner's, and who may read a row is the app's decision.
- **A browser `track` call with a free payload.** Rejected because the bundle is inspectable and replayable, the same reason ADR-0001 keeps SQL off the client.
- **Let Server Actions query the Kernel store.** v1 leaves that out, so in-app funnels stay in SQL.

## Consequences

Export with live state takes the Usage store along with accounts, the database and files. The Validation Gate and the Kernel both parse `signal` on an action and `kind: "signal"`. A Signal counts toward the action cap. The store has its own size bound and drops the oldest rows. The Development Kernel records the same way so an unpublished app still shows Usage.
