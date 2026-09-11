# Run External Projects against a local Development Kernel

Status: accepted

An External Project will develop against a local implementation of the Kernel interfaces instead of embedding production credentials or depending on live production state. Application code keeps using the same relative Runtime Interface for actions, auth and files in local preview and hosted Deployments. Remote production administration remains an explicit owner action, never an implicit part of the development runtime.

## Decision

The local toolchain routes Littlebox runtime requests to a Development Kernel. It shares the production Server Action parser, authorization policy and capability semantics, while local adapters provide a persistent development database, simulated Runtime Actors and local file storage. Browser code receives neither raw database access nor an administrative credential; it continues to call named Server Actions and the file/auth interfaces through `@littlebox/sdk`.

The normal development loop is local and cannot mutate production:

```text
Local app -> Development Kernel -> local database / actors / files
```

An explicit cloud-development mode may create a short-lived Preview with remote auth and storage, optionally initialized from an owner-authorized snapshot. Preview credentials stay in a local gateway or trusted tooling and never enter application JavaScript. Copying production data into that Preview is a separate, auditable owner operation.

Littlebox will not transparently proxy ordinary development requests into a live Deployment. An External Project Owner may separately open a short-lived, owner-authenticated Admin Interface connection for schema administration from local Drizzle tooling. Littlebox keeps that connection out of the Runtime Interface and browser, records the operation and takes infrastructure-level precautions such as backup and quotas; the External Project Owner remains responsible for destructive changes, drift and compatibility under ADR-0004 and ADR-0005. Managed Projects never receive this escape hatch.

## Considered options

- **Use ad hoc mocks for each capability.** Rejected because separate database, auth and file mocks would drift from Kernel policy. One Development Kernel preserves a single interface and concentrates parity work.
- **Connect local apps directly to production.** Rejected because routine edits and tests could mutate live state, require privileged connectivity and become dependent on network state.
- **Require a remote Preview for every edit.** Rejected as the default because it adds latency, availability and infrastructure cost to a workflow that SQLite, simulated actors and local files can reproduce. Remote Preview remains an explicit integration mode.

## Consequences

The Kernel's action parsing and authorization implementation must be reusable by hosted and development adapters. Littlebox must provide local persistence, actor switching, file handling, reset/seed/export tools, Preview lifecycle and secure owner administration for External Projects. Local and hosted storage implementations may vary, but tests must exercise the same Kernel interface so a capability cannot behave differently merely because the app was ejected.
