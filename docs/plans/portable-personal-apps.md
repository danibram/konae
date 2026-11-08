# Prototypes with a portable mini kernel

Proposal revised on October 11, 2026. It does not change the accepted ADRs or implement new features.

## Goal and recommendation

The user describes an app to the agent, tries the result in the browser and publishes a public URL. The app can have login, internal areas and private data. Each app has its own database, users and permissions. The creator can keep using Konae, self-host the same mini kernel or replace its capabilities with their own code.

I recommend a shared kernel that runs actions on one active SQLite per app and environment, stored on a persistent volume. The frontend is a static artifact. Backups and export materialize the database as portable objects. The object store supports filesystem, S3 and later GCS, without taking part in each write.

The portable database format and how the database runs are separate decisions. A SQLite can be exported as an object and, while it is active, receive normal kernel transactions. There is no need to upload it on every mutation or to turn object storage into the transaction coordinator.

## Critique of the options

The criteria are time to the first prototype, operational load, ease of development for the agent and the cost of leaving Konae.

| Option | What it solves | What it complicates | Decision |
| --- | --- | --- | --- |
| Active SQLite with separate backups | Normal writes, a small kernel and export as a standard file | Requires a persistent volume; losing it can lose changes made after the backup | Choose for v1 |
| Active SQLite with incremental replication | Reduces the remote copy lag without uploading the whole database per write | Adds log management, restore and monitoring | Evaluate when a measured recovery need exists |
| Full snapshot and remote head per mutation | Makes the object store the authority for every committed state | Cost proportional to DB size, remote latency and uncertainty on commit | Discard for this initial product |
| Arbitrary backend per app | Allows any server code and library | Processes, isolation, dependencies and deployments per app | Reserve as an external exit |
| Public database downloaded in the browser | Serves a read-only dataset with static files | Does not enforce row-level permissions on downloaded data or coordinate users who write | Only for deliberately public datasets |

The first option has real limits. A machine going down makes actions and login temporarily unavailable. A lost disk requires a restore. An app with slow queries can consume shared resources. These limits are handled with verifiable backups, query limits and one responsible instance per database, not with a distributed protocol in the first version.

Incremental replication can rely on existing tools. Litestream replicates SQLite changes in the background and keeps recovery material. It does not turn the application into a multi-writer system, and on its own it does not guarantee that every local commit is already at the remote destination. [How Litestream works](https://litestream.io/how-it-works/).

## Path of a request

```text
Browser -> action + input -> shared kernel -> SQLite for that app
                                  |
                                  +-> session and permissions for that app

Static server / CDN -> HTML, JS, CSS and assets
Backup process -> consistent snapshot -> filesystem / S3 / GCS
```

1. The browser sends an action name and its parameters.
2. The kernel resolves the app from the domain or a configured route, and the user from their session.
3. It checks the declaration, permissions, input and limits.
4. It runs parameterized SQL inside a transaction on the matching database.
5. It responds after the commit. It does not wait for S3 or copy the whole database.

The browser receives no database credentials and cannot pick another user or SQLite file. Anonymous state must be represented explicitly.

A queue per database can coordinate mutations, migrations and restores without blocking other apps. SQLite WAL allows concurrent readers with one writer. The volume and the sync configuration must uphold the local commit guarantee. The same database is never opened from independent replicas on several machines. [SQLite WAL](https://www.sqlite.org/wal.html).

## Persistence and objects

| Element | While the app runs | Portable object |
| --- | --- | --- |
| Frontend | Static server, disk or CDN | Immutable artifact |
| Data | Active SQLite on a persistent volume | Consistent SQLite snapshot |
| Auth and permissions | Identity store isolated per app | Export of that app's users and permissions |
| Files | Disk or object store | Blobs plus inventory |
| Model and changes | Accepted schema and migration history | Source and versioned SQL |

The backup runs when the owner requests it, before migrations and at a configured frequency. The panel shows the date and revision of the last completed backup. If a backup fails, the age of the recoverable point grows. A write that was already committed does not become an error.

A scheduled interval does not guarantee the same loss bound if copies fail. The real recoverable point is the last valid backup. A process restart keeps the database on the volume; losing the volume is a different failure. A container with an ephemeral disk does not meet this proposal.

The snapshot uses a consistent SQLite operation, such as the Online Backup API or VACUUM INTO. Nobody blindly copies only the main file of an active WAL database. Backups have retention and a restore test. [SQLite Backup API](https://www.sqlite.org/backup.html).

The first full export can briefly pause the app's writes and identity changes to copy data, users, permissions and file references at a coordinated point. A restore must close earlier connections, validate the copy and activate the restored state. An old remote copy must not be restored automatically over a healthy volume.

## A small contract for the agent

The agent works with app files, a data model, declared actions and permissions. It does not need to pick drivers, configure storage or install a new backend for each prototype.

```text
index.html
styles.css
app.js or src/main.ts
schema.ts
actions.json
konae.manifest.json
migrations/                # reviewable lifecycle output
```

The proposal keeps Drizzle as the model, per ADR-0005. Going back to schema.sql as a second authority now would change another decision without solving the persistence problem. The generated SQL stays available for inspection and export. The first templates and agent tools should cover a small subset of the model.

The agent has a small SDK. These methods are the proposed contract; today some capabilities still differ between the preview and the kernels.

```js
await app.auth.session();
await app.auth.signIn();
await app.auth.signOut();
await app.actions.call('notes.create', { title: 'Groceries' });
await app.files.upload(file);
```

The editor provides the initial login form, user simulation and templates for common actions. The agent changes the app code and proposes model and permission changes. The gate checks build, schema, migration and actions before applying. The platform handles administration; the agent never gets an administrative key inside the generated code.

There are three possible designs for data access.

| Design | Advantage | Critique |
| --- | --- | --- |
| Named actions with parameterized SQL | Already exists, keeps each operation in one place and lets you replace it by name | Has limits for complex logic; the SQL must check row access |
| Generic CRUD from the client | Reduces code in simple demos | Forces the design of general filters, joins and policies; turns the platform into a table access engine |
| Free-form server JS functions | Allows conventional logic | Running user code requires extra isolation and operations |

I would keep the existing actions. For a common pattern that turns out verbose, I would add generation or a reviewable template. I would not extend actions.json until it becomes a general-purpose language. An operation that needs several writes must be atomic in the backend. If the initial format does not support it, that limitation is stated and is not simulated with several browser calls.

## Permissions and auth

The app entry is public. Internal areas ask for a session, and the server authorizes every protected operation. The public frontend never includes private data or secrets.

Each app has its own users and sessions, separate from the editor accounts. The kernel shares the auth implementation without sharing identity between apps. The initial roles can be limited to app owner and member, with public, authenticated and restricted actions.

The session identifies the actor, but it does not make every query safe. An action that returns private notes must filter by the actor obtained from the kernel. A mutation must apply the access condition to the record it changes. Validation needs cases with two users and two apps, on top of valid SQL. Nobody should announce automatic row isolation just because the SQL uses parameters.

The auth store must not be reachable through action SQL. I would start with a separate identity file per app, managed by the kernel. That simplifies isolation and requires coordinating it with the database on export. The initial login mechanism should reuse a maintained implementation. I would not build several providers or flows before finishing one.

## Abstractions and exit

Two public deliverables are enough at first.

- An importable JavaScript SDK, with auth, actions and files modules.
- The same mini kernel in a self-hostable distribution with configuration, volume and admin tools.

Model, migrations, export and adapters can start as internal modules. There is no need to publish a library per concept. The editor and the self-hosted distribution must use the same tested contract.

The SDK can keep its configuration in a single app file. The example shows a future interface.

```ts
// src/platform.ts
import { createClient } from '@konae/sdk';

export const app = createClient({ baseUrl: '/_konae' });
```

A same-origin proxy can route that prefix to the hosted kernel or to a self-hosted one. Changing a URL across domains and assuming cookies will work is not enough; the self-hosted distribution includes that configuration.

| Step | Work | What keeps working |
| --- | --- | --- |
| Export code | Generate a runnable project with pinned dependencies and scripts | The app can keep consuming Konae |
| Self-host mini kernel | Import data, users, permissions and files; change the runtime connection | Same action names and frontend calls |
| Replace an action | Implement its input, output and permissions in a custom handler | The frontend keeps that call |
| Replace auth | Connect another session validation and login, keeping user IDs | Data keeps its owner if the mapping is correct |
| Replace SQLite | Migrate data and review SQL, model and transactions | Action contracts can be kept |
| Leave completely | Serve your own frontend and backend | No Konae account or connection is required |

Handler replacement is a capability of the external backend. It does not authorize running arbitrary JS inside the shared kernel. A first implementation can use an external router that serves the replaced actions and forwards the rest to the mini kernel, keeping validated identity and equivalent controls. It does not accept an actor declared by the browser. If an operation needs data from two backends, no distributed transaction is promised.

Switching auth requires mapping identities in a verified way. It can force users to sign in again or reset passwords. Switching to PostgreSQL can require rewriting SQL. The promise is to locate the dependencies and keep the app working where the contract still holds. It does not make every migration free.

Self-hosting does not force you to replace anything. It should be the easiest exit. A Managed Project can keep being edited in Konae while using a compatible self-hosted mini kernel. After ejecting code authority, the repository becomes the source and Konae can keep publishing artifacts. There must not be two independent writers of the same code.

## What to reuse from the current code

- Keep the editor, the agent working on a working copy, the gate, the action parser, the runtime bridge, the WASM build and SQL Studio.
- The Rust Kernel already runs the action, authorization, SQLite transaction and response path without uploading a snapshot per mutation. See `invoke_action` and `run_mutation` in `kernel-rs/src/main.rs`.
- The Rust Kernel no longer uploads the database after each commit, so a remote failure cannot fail a write that already happened. Databases are not replicated to S3 at all yet; that replication still has to be built outside the request path.
- Writes are coordinated per database (`database_lock` in `kernel-rs/src/main.rs`), so independent apps do not serialize each other.
- Separate app identity from publishing. Changing code must not create another database or other users.
- Extract an SDK from the injected miniapp, keeping compatibility during the transition.
- Turn the current JSON export into a runnable package plus an authorized data package.
- Persistent per-app users exist in the Rust Kernel, with hashed sessions and a per-Deployment signup policy.

Update, 2026-10-23: the Bun kernel server has been removed, and the Rust Kernel in `kernel-rs/` is the only Kernel. The original advice stands: prove the correct path on the Kernel that Docker runs, separately from the app contract.

## Limits I would accept in v1

- Personal apps with few users and bounded traffic.
- One active instance responsible for each database, and a mandatory persistent volume.
- Static frontend, declared actions and approved browser packages.
- Limits on query time, rows, input, database size and files.
- No arbitrary backend execution, per-app processes, automatic failover or multi-region writes.
- One complete login method and simple permissions, not an organizations system.
- Backups with visible age, with no zero-loss promise if the volume is destroyed.
- Small, previewed and validated migrations, with recovery in place before applying.

## Sequence and acceptance tests

| Delivery | Bounded work | Test that decides it is done |
| --- | --- | --- |
| 1. Publish and iterate without losing data | Stable database per app/environment, local transaction and separate backup | Creating a row, publishing v2 and restarting the kernel keeps that row; a remote backup failure does not turn its write into an error |
| 2. Useful app with auth | Persistent user, public and internal areas, per-actor actions | Two users and two apps cannot access each other's data; login and revocation work |
| 3. Change the model and recover | A Drizzle model, validated migration, backup and restore | A migration keeps rows; a failed one leaves a defined state; a copy restores into a clean directory |
| 4. Run elsewhere | SDK and app package, self-hostable mini kernel and import | The app works on another installation with Konae unreachable and keeps users and permissions |
| 5. Replace a capability | External implementation of one action with the same contract | Replacing notes.create keeps the frontend and the access tests; the rest keeps using the kernel |

The full demo starts with a request to the agent to create a notebook with a public cover page, login, private notes and a shared list. It continues with a model edit, a second publish and the export. It ends with that same notebook running on the self-hosted mini kernel. Publishing and eject are designed for that path before adding more infrastructure providers.

## Delegation and open decisions

The main agent sets contracts and reviews integration. Lower-cost agents can extract the SDK, build the exporter and add behavior tests in separate tasks. Identity, migration and persistence changes require a joint review of their invariants. An open-ended backend rewrite is not delegated.

The ADRs must record a stable database per app and environment, local commit separate from remote backup, per-app auth, self-hosting of the same runtime and external replacement of capabilities. This document proposes those decisions. Describing them does not change the guarantees of the existing code.
