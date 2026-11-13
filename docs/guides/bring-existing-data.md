# Bring existing data

Load rows and files from another system into the app folder's `data/`, try them on `konae dev`, then send them to the published app with `konae push`.

`push` replaces the published data. Rows are never merged, so `data/` must hold everything the published app should keep.

## What data/ holds

```text
data/
  database.sqlite   the app's tables, as app/schema.sql declares them
  identity.sqlite   the app's accounts; the Kernel writes it
  files/
    index.json      one entry per uploaded file
    <id>.blob       the bytes of each file, named by its id
  usage.sqlite      Usage and Signals; the Kernel writes it
```

`secrets.sqlite` may sit there too. `push` never sends it.

## 1. Let the Kernel make the files

Run `dev` once, then stop it:

```sh
bun run konae -- dev --dir ./my-app
```

The Kernel writes `data/database.sqlite` with the tables of `app/schema.sql`, and `data/identity.sqlite` with the accounts tables. Start from these files rather than writing SQLite files from scratch, because `push` and the Kernel check that `identity.sqlite` has an `app_users` table and that the database holds no trigger or view.

## 2. Load the rows

Insert the rows with the `kernel` binary's `sql` command, which `konae sql` runs on `data/database.sqlite`:

```sh
bun run konae -- sql --dir ./my-app --file ./import.sql
```

Keep to the tables and columns of `app/schema.sql`. When the published schema lacks a column that `data/` holds, the import would drop it, and `push` stops with the plan until you confirm it.

Accounts come from the app, not from a table you write. Create them on `konae dev` through sign-up or `kernel invite`, or start from a `konae pull` of the published app, so their ids are the Kernel's own. A row that names its owner keeps that account's id, the value the `actor` binding gives a Server Action.

## 3. Add the files

Put each file's bytes in `data/files/<id>.blob` and list it in `data/files/index.json`:

```json
[
  {
    "id": "6f1c2a7e-3b4d-4e5f-8a9b-0c1d2e3f4a5b",
    "name": "menu.pdf",
    "type": "application/pdf",
    "size": 48213,
    "visibility": "public",
    "createdAt": 1794535700000
  }
]
```

- `id` is 36 characters of hexadecimal digits and dashes, such as a UUID, and names the blob.
- `size` is the blob's size in bytes, exactly.
- `visibility` is `public` or `private`.
- `ownerId`, when present, is the id of an account in `identity.sqlite`. Leave it out for a file no account owns.
- `createdAt` is milliseconds since 1970.

Each file must fit the installation's per-file limit, and all files together its quota: 2 MiB and 20 MiB unless the operator raised `KONAE_FILE_MAX_BYTES` and `KONAE_FILES_QUOTA_BYTES`. `platform_describe` answers both as `files.maxFileBytes` and `files.quotaBytes`. Anything in `data/files` that the index does not list stays behind.

## 4. Try it, then push

Run `dev` again and use the app on its own data. Then compare it with the published app:

```sh
bun run konae -- push --dir ./my-app
```

That prints the rows of each table here against the published rows, what happens to accounts, files and Usage, and sends nothing. A file past a limit is refused here, by name, before anything leaves. When the numbers are right:

```sh
bun run konae -- push --dir ./my-app --yes
```

The app goes into maintenance, the import backs up the published data, and the app comes out of maintenance. `push` prints the backup's id; `database_execute` with `operation` `restore-backup` and that `backupId` brings the earlier data back.

## Read next

- [CLI](../cli.md#push)
- [Upload files](./upload-files.md)
- [Take the app elsewhere](./take-the-app-elsewhere.md)
- [Limits](../limits.md)
