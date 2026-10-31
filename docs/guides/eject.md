# Eject a managed project

Hand a Managed project's source to its owner as a folder on disk, while its Deployment, URL, accounts and data stay where they are.

## What changes

Eject is irreversible. The project becomes External. The Kernel records who ejected it and when, refuses every Managed source change from every tab, agent and credential, and keeps accepting Build Artifacts. The Deployment's URL, accounts, sessions, data, invitations and email continue. Eject exports no data. To edit in the editor again, create a new project.

A project you made with `konae init` is already External, and `eject_plan` refuses it. If you want to move data, use a [package](./take-the-app-elsewhere.md) instead.

## Steps in the editor

1. On the project list, find the **Take it with you** group. It lists each Managed project with an **Eject** action.
2. Press **Eject** for the project. The editor downloads the code as a tar and checks it, and only then ejects.
3. The project opens read-only with the commands to publish from outside.

## Steps with the CLI

1. Log in with `manage_lifecycle`. The read, write, publish, manage_users and export_data permissions do not include it.

    ```sh
    KONAE_EMAIL=you@example.com KONAE_PASSWORD=... \
    bun run konae -- login --dir /tmp/ejected --url http://localhost:4173 --manage-lifecycle
    ```

2. Run `eject` into an empty folder, naming the project's ID. It plans at the revision the server last saved, or at `--revision`. It downloads the package, checks every file's SHA-256, writes the files and a `konae.json`, and only then applies.

    ```sh
    bun run konae -- eject --dir /tmp/ejected --project <project id> --url http://localhost:4173
    ```

3. Publish from the folder. The Deployment, its URL and its data stay.

    ```sh
    bun run konae -- publish --dir /tmp/ejected
    ```


A stale revision is refused before anything downloads. After an interruption, run the same `eject` command again in the same folder. The receipt is saved in `.konae/eject-receipt.json`.

## Over MCP

Both calls need `manage_lifecycle`. `eject_plan` takes `projectId` and `expectedRevision`, the saved-sources revision that `project_inspect` returns as `sourcesRevision`. It answers a `planId`. `eject_apply` takes that `planId`, checks it is still current and records the Eject.

## Check it worked

- The read-back for `eject_plan` is `eject_plan` itself. The read-back for `eject_apply` is `project_inspect`. It returns the Deployment and its URL, which keeps serving. The project's source authority now reads external.
- Open the Deployment URL and use one feature that reads a row. The accounts and rows are unchanged.
- `project_sources_read` still returns the frozen sources, marked `readOnly: true`.
- After you publish from the folder, the read-back for `release_apply` is `project_inspect`.

The editor preview cannot show an Eject. It has no Deployment to keep serving, so check on the published app.

## Serve the app on its own domain

Eject leaves the app on the installation it was made on. There are two ways to give it a domain of its own, and both keep the same Kernel API: the same Server Actions, the same Validation Gate and the same row limit.

- **Stay on the installation.** Give the Deployment a custom domain with `bun run konae -- domain --dir /tmp/ejected --deployment <id> --set notes.example.org` or `domain_set`, add the TXT record it prints, and run it again. The installation's operator routes the name to the Kernel and terminates TLS for it.
- **Run it on your own machine.** Export a self-host kit with the app's data and run its Kernel behind your own proxy. The rest of this section is this path.

The page and its Server Actions answer from the same host. The SDK calls `/_konae/mcp` on the page's own host and the Kernel refuses an `Origin` that is not the app's, so you cannot serve the page from another host and call the Kernel across origins.

1. Eject, as above, so the code is yours to change.
2. Export the kit with the live data. The credential needs `export_data`.

    ```sh
    bun run konae -- login --dir /tmp/ejected --url http://localhost:4173 --export-data
    bun run konae -- export --dir /tmp/ejected --out /tmp/notes-kit.tar --with-data
    ```

    To leave the installation for good, so the origin stops taking writes, run a [migration](./take-the-app-elsewhere.md#migrate-with-one-writer-at-a-time) instead.
3. On the new machine, unpack the kit and write `.env` before the first deploy. The Kernel names the app's generated address when it creates the app, so these names have to be set first.

    ```sh
    KONAE_PORT=8787
    KONAE_ADMIN_KEY=<a long random key>
    KONAE_PUBLIC_ORIGIN=https://konae.example.org
    KONAE_HOST_SUFFIX=apps.example.org
    KONAE_APP_DOMAIN=notes.example.org
    ```

    `KONAE_PUBLIC_ORIGIN` is the Kernel's own host. Owner sign-in answers only there, and the deploy script names it while it talks to the Kernel on 127.0.0.1, so the deploy does not need it to be public. `KONAE_HOST_SUFFIX` is where generated app addresses go. `KONAE_APP_DOMAIN` is the app's hostname, which cannot be the Kernel's host or sit under the suffix. An `https` origin turns on `__Host-` session cookies and HSTS.
4. Point `notes.example.org` at the machine and put a proxy in front of the Kernel. It terminates TLS for the app's hostname and forwards to `127.0.0.1:8787` with the Host header unchanged. The Kernel issues no certificate. `ops/nginx.local.conf` shows a second host reaching the Kernel.
5. Run `./start.sh`. It imports the accounts, rows and files, publishes the code and prints a TXT record, `_konae.notes.example.org` with a token this Kernel issued. A token from the old installation does not carry over.
6. Add the record at your DNS provider and run `npm run deploy` again. A value the old installation needed can stay in the same record; the run reports that it found another value and prints the record again until yours is there. Once the Kernel finds the token, the script prints `The app also answers at https://notes.example.org.`

After a change to the code under `app/`, run `npm run build` and `npm run deploy`. The rows and the domain stay.

Sessions, email links, live API keys and signing secrets do not travel. People sign in again with their passwords. Sessions do not cross between the generated address and the app's hostname either, so a person signs in on each.

To issue new API keys or set signing secrets with `secret_set`, sign in as the kit's owner on the Kernel's own host. That host then has to reach the Kernel through your proxy as well, because the Kernel refuses owner sign-in on any other host, `127.0.0.1` included. The owner is `owner@selfhost.local` and the password is the setup key.

```sh
KONAE_EMAIL=owner@selfhost.local KONAE_PASSWORD=<setup key> \
bun run konae -- login --dir /tmp/notes-admin --url https://konae.example.org
```

For an app whose HTTP routes check a signature, `npm run deploy` stops at `release_plan` with the name of each secret it needs. Set them this way and run `npm run deploy` again.

### Check it worked

- The last `npm run deploy` prints the app's hostname as an address it answers at. Before the TXT record is found it prints the record again.
- Open `https://notes.example.org`, sign in with an account from the old installation and open one row it wrote there.
- `tests/self-host-domain.integration.test.ts` runs the whole path on two local Kernels with a DNS stub.

## Read next

- [Edit the app's files with the CLI](./edit-files-with-the-cli.md)
- [Take the app elsewhere](./take-the-app-elsewhere.md)
- [CLI reference](../cli.md)
- [Concepts](../concepts.md)
