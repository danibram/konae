# Take the app elsewhere

Export a package to move one app's accounts, data and rules to another machine, or run a migration when exactly one installation must take the writes.

## Pick the right move

| You want | Use | Why |
| --- | --- | --- |
| A copy of one app on another machine | Package export and import | It moves one Deployment's database, accounts, files and, when present, Usage and Signals. |
| The app to leave and the old one to stop | Migration | It pauses the origin, then retires it when you confirm, so one installation takes the writes. |
| To move a whole installation | Copy | A copy moves an installation, not one app. It is operator work, not an app feature. |

A package never moves sessions, email links or live API keys. People sign in again with their passwords, and each API key arrives revoked, so you issue new ones. Signing secrets do not travel either, so set them again on the destination.

## Export a package

1. Log in with `export_data`.

    ```sh
    KONAE_EMAIL=you@example.com KONAE_PASSWORD=... \
    bun run konae -- login --dir /tmp/my-app --url http://localhost:4173 --export-data
    ```

2. For a self-hosted kit that includes the live data, run `export` with `--with-data`. It refuses to overwrite an archive. Exports with live state hold password hashes and uploaded files, so keep them private.

    ```sh
    bun run konae -- export --dir /tmp/my-app --out /tmp/my-app-with-data.tar --with-data
    ```

3. Or over MCP, call `package_export` with the `deploymentId`. It returns a manifest and one download per component, `database`, `identity`, `files` and, when the app recorded any, `usage`.
4. Read each download with `transfer_read` and its `transferId` and `offset` until `nextOffset` equals `size`. Check its SHA-256, then call `transfer_cancel`.

## The self-host kit

Export project downloads a tar archive with source files, a built frontend, the Rust Kernel and startup scripts. For a published app it also includes the database, users and uploads. Imported users keep their password hashes, while sessions are cleared. The editor still builds this archive in the browser in the small format, so its combined state limit is 16 MiB and each file is limited to 2 MiB; the CLI below has no such limit. The archive documents two ways to run it: Docker, or native Cargo with Rust 1.91 or newer. Its `deploy.mjs` publishes the app as private unless the exported project manifest sets `visibility` to `"public"`. The kit starts on localhost. To serve the app on a domain you control, set its names in `.env` before the first deploy, as [Serve the app on its own domain](#serve-the-app-on-its-own-domain) describes.

`--with-data` downloads the app as a package. It holds a manifest and one file each for the database, the accounts and the files, streamed in 64 KiB blocks, so an app at its 256 MiB database and its file quota exports whole. The kit's `deploy.mjs` uploads the package the same way and the target Kernel checks it before it creates the app. [The MCP guide](../mcp.md#export-and-import-an-app) describes the format. The command refuses to overwrite an archive and writes it with owner-only permissions. Live-state exports retain the deployed visibility and signup policy. The target installs that policy with its first deployment. Exports with live state contain password hashes and attachments; keep them private. Extracted snapshots have owner-only permissions and the kit's Git rules exclude them.

## Serve the app on its own domain

An app runs on the installation it was published on. There are two ways to give it a domain of its own, and both keep the same Kernel API: the same Server Actions, the same Validation Gate and the same row limit.

- **Stay on the installation.** Give the Deployment a custom domain with `bun run konae -- domain --dir /tmp/notes --deployment <id> --set notes.example.org` or `domain_set`, add the TXT record it prints, and run it again. The installation's operator routes the name to the Kernel and terminates TLS for it.
- **Run it on your own machine.** Export a self-host kit with the app's data and run its Kernel behind your own proxy. The rest of this section is this path.

The page and its Server Actions answer from the same host. The SDK calls `/_konae/mcp` on the page's own host and the Kernel refuses an `Origin` that is not the app's, so you cannot serve the page from another host and call the Kernel across origins.

1. Export the kit with the live data. The credential needs `export_data`.

    ```sh
    bun run konae -- login --dir /tmp/notes --url http://localhost:4173 --export-data
    bun run konae -- export --dir /tmp/notes --out /tmp/notes-kit.tar --with-data
    ```

    To leave the installation for good, so the origin stops taking writes, run a [migration](#migrate-with-one-writer-at-a-time) instead.
2. On the new machine, unpack the kit and write `.env` before the first deploy. The Kernel names the app's generated address when it creates the app, so these names have to be set first.

    ```sh
    KONAE_PORT=8787
    KONAE_ADMIN_KEY=<a long random key>
    KONAE_PUBLIC_ORIGIN=https://konae.example.org
    KONAE_HOST_SUFFIX=apps.example.org
    KONAE_APP_DOMAIN=notes.example.org
    ```

    `KONAE_PUBLIC_ORIGIN` is the Kernel's own host. Owner sign-in answers only there, and the deploy script names it while it talks to the Kernel on 127.0.0.1, so the deploy does not need it to be public. `KONAE_HOST_SUFFIX` is where generated app addresses go. `KONAE_APP_DOMAIN` is the app's hostname, which cannot be the Kernel's host or sit under the suffix. An `https` origin turns on `__Host-` session cookies and HSTS.
3. Point `notes.example.org` at the machine and put a proxy in front of the Kernel. It terminates TLS for the app's hostname and forwards to `127.0.0.1:8787` with the Host header unchanged. The Kernel issues no certificate. `ops/nginx.local.conf` shows a second host reaching the Kernel.
4. Run `./start.sh`. It imports the accounts, rows and files, publishes the code and prints a TXT record, `_konae.notes.example.org` with a token this Kernel issued. A token from the old installation does not carry over.
5. Add the record at your DNS provider and run `npm run deploy` again. A value the old installation needed can stay in the same record; the run reports that it found another value and prints the record again until yours is there. Once the Kernel finds the token, the script prints `The app also answers at https://notes.example.org.`

After a change to the code under `app/`, run `npm run build` and `npm run deploy`. The rows and the domain stay.

Sessions, email links, live API keys and signing secrets do not travel. People sign in again with their passwords. Sessions do not cross between the generated address and the app's hostname either, so a person signs in on each.

To issue new API keys or set signing secrets with `secret_set`, sign in as the kit's owner on the Kernel's own host. That host then has to reach the Kernel through your proxy as well, because the Kernel refuses owner sign-in on any other host, `127.0.0.1` included. The owner is `owner@selfhost.local` and the password is the setup key.

```sh
KONAE_EMAIL=owner@selfhost.local KONAE_PASSWORD=<setup key> \
bun run konae -- login --dir /tmp/notes-admin --url https://konae.example.org
```

For an app whose HTTP routes check a signature, `npm run deploy` stops at `release_plan` with the name of each secret it needs. Set them this way and run `npm run deploy` again.

### Check the domain

- The last `npm run deploy` prints the app's hostname as an address it answers at. Before the TXT record is found it prints the record again.
- Open `https://notes.example.org`, sign in with an account from the old installation and open one row it wrote there.
- `tests/self-host-domain.integration.test.ts` runs the whole path on two local Kernels with a DNS stub.

## Import on the destination

1. Run `project_create` with the same `projectId`. Then run `secret_set` for every secret the app's routes sign with. A release that signs with an unset secret is refused.
2. For each component, call `transfer_begin` with `size`, `sha256`, `projectId` and the component's name. Call `transfer_write` for each block, then `transfer_complete`.
3. Build the folder and upload the release JSON, with `initialPackage` holding the `manifest` and a `components` map of component name to `transferId`. Then call `release_plan` and `release_apply`. An import installs only into a project that has no Deployment yet. The same package imported again returns the first import.
4. Issue new API keys. `api_keys_list` shows each key revoked at import.

## Migrate with one writer at a time

A migration needs `manage_lifecycle` and `export_data` on the origin and `manage_lifecycle` on the destination. Log in with both flags.

```sh
bun run konae -- login --dir /tmp/my-app --url http://localhost:4173 --export-data --manage-lifecycle
```

1. Call `migration_begin` with the origin's `deploymentId`. The app pauses at once and shows a maintenance page. It takes no writes, signups, sign-ins, invitations or HTTP routes. The answer holds the package downloads and the next steps.
2. Import on the destination as above, adding the release download as the release payload. The imported app stays in maintenance. Check it with read tools.
3. Call `migration_confirm` on the origin with `operationId` and, if the address changed, `newUrl`. The origin stays retired and shows a link to the new address for seven days. Keep the `confirmation` it returns.
4. Call `migration_activate` on the destination with `deploymentId` and the `confirmation`. Add `resendInvitations` to send pending invitations again from the new host.
5. To stop, call `migration_cancel` with `operationId` before `migration_confirm`. It resumes the origin. After a confirm, cancel is refused.

## Check it worked

- After `package_export`, `project_export` or `database_export`, the read-back is `transfer_read`. Verify the size and SHA-256 of every component.
- After `transfer_begin`, `transfer_write` or `transfer_complete`, the read-back is `transfer_status`. Bytes in an open transfer are not a Deployment.
- After `release_apply` on the destination, the read-back is `project_inspect`.
- After any `migration_*` call, the read-back is `migration_status`. A `service_timeout` or interrupted call must be verified the same way before you retry.
- On the destination, sign in with an existing password and open one row the app owned.

The editor preview is not a Deployment, so it has no database to package. Test the whole move on two `konae dev` Kernels or two real installations. The repository's migration rehearsal runs the procedure on two local Kernels with `bun run tests/smoke/migration.ts --port 19460`.

## Read next

- [Invite people](./invite-people.md)
- [Pick the smallest piece](../pieces.md)
- [Concepts](../concepts.md)
